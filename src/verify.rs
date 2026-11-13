use std::collections::HashSet;
use std::fs;
use std::path::Path;
use walkdir::WalkDir;

use crate::history::{
    is_flat_repo, list_snapshots, load_baseline, load_flat_manifest, load_manifest,
};
use crate::models::{FileState, SnapshotManifest};
use crate::util::{
    compute_file_crc32, format_size, get_file_metadata, is_reserved_flat_name,
    is_safe_relative_path, normalize_rel_path,
};

#[derive(Debug, Clone)]
pub struct VerifyOptions {
    pub quick: bool,
    pub verbose: bool,
    pub target_time: Option<String>,
}

#[derive(Default, Debug)]
pub struct VerifyResult {
    pub verified_ok: usize,
    pub total_bytes_verified: u64,
    pub corrupted: Vec<CorruptedFile>,
    pub missing: Vec<String>,
    pub untracked: Vec<String>,
}

#[derive(Debug)]
pub struct CorruptedFile {
    pub path: String,
    pub reason: String,
}

pub fn run_verify(opts: &VerifyOptions, dest: &Path) -> Result<(), String> {
    if !dest.exists() {
        return Err(format!(
            "Destination directory '{}' does not exist.",
            dest.display()
        ));
    }

    let is_flat = is_flat_repo(dest);
    let mut result = VerifyResult::default();

    println!(
        "Verifying backup integrity at: {} (mode: {}, method: {})",
        dest.display(),
        if is_flat { "flat" } else { "standard" },
        if opts.quick {
            "quick (size/mtime)"
        } else {
            "full (CRC32 recalculation)"
        }
    );

    if is_flat {
        verify_flat(opts, dest, &mut result)?;
    } else {
        verify_standard(opts, dest, &mut result)?;
    }

    // Print report
    println!("\nVerification Summary:");
    println!("---------------------");
    println!(
        "Verified OK:        {} files ({})",
        result.verified_ok,
        format_size(result.total_bytes_verified)
    );

    if !result.missing.is_empty() {
        println!("Missing Files:      {} file(s)", result.missing.len());
        for m in &result.missing {
            println!("  [MISSING]   {}", m);
        }
    } else {
        println!("Missing Files:      0");
    }

    if !result.corrupted.is_empty() {
        println!(
            "Corrupted Files:    {} file(s) [BIT-ROT DETECTED]",
            result.corrupted.len()
        );
        for c in &result.corrupted {
            println!("  [CORRUPTED] {}: {}", c.path, c.reason);
        }
    } else {
        println!("Corrupted Files:    0");
    }

    if !result.untracked.is_empty() {
        println!("Untracked / Orphan: {} file(s)", result.untracked.len());
        if opts.verbose {
            for u in &result.untracked {
                println!("  [UNTRACKED] {}", u);
            }
        }
    } else {
        println!("Untracked / Orphan: 0");
    }

    if !result.corrupted.is_empty() || !result.missing.is_empty() {
        return Err(format!(
            "Integrity check FAILED: {} corrupted, {} missing file(s).",
            result.corrupted.len(),
            result.missing.len()
        ));
    }

    println!("\nIntegrity check PASSED: all files intact and verified.");
    Ok(())
}

fn verify_file_state(
    file_on_disk: &Path,
    display_path: &str,
    state: &FileState,
    opts: &VerifyOptions,
    result: &mut VerifyResult,
) {
    if !file_on_disk.exists() {
        result.missing.push(display_path.to_string());
        return;
    }

    let meta = match fs::metadata(file_on_disk) {
        Ok(m) => m,
        Err(e) => {
            result.corrupted.push(CorruptedFile {
                path: display_path.to_string(),
                reason: format!("Failed to read metadata: {}", e),
            });
            return;
        }
    };

    if meta.len() != state.size {
        result.corrupted.push(CorruptedFile {
            path: display_path.to_string(),
            reason: format!(
                "Size mismatch: expected {} bytes, found {} bytes",
                state.size,
                meta.len()
            ),
        });
        return;
    }

    if opts.quick {
        if let Ok((_, actual_mtime_ns, _)) = get_file_metadata(file_on_disk) {
            // Allow up to 2 seconds (2_000_000_000 ns) tolerance for FAT32/exFAT timestamp rounding
            if actual_mtime_ns.abs_diff(state.mtime_ns) > 2_000_000_000 {
                result.corrupted.push(CorruptedFile {
                    path: display_path.to_string(),
                    reason: format!(
                        "Timestamp (mtime) mismatch (>2s): expected {}, found {}",
                        state.mtime_ns, actual_mtime_ns
                    ),
                });
                return;
            }
        }
        result.verified_ok += 1;
        result.total_bytes_verified += state.size;
        if opts.verbose {
            println!("  [OK] {}", display_path);
        }
        return;
    }

    match compute_file_crc32(file_on_disk) {
        Ok(actual_crc) => {
            if let Some(expected_crc) = state.crc32 {
                if actual_crc != expected_crc {
                    result.corrupted.push(CorruptedFile {
                        path: display_path.to_string(),
                        reason: format!(
                            "CRC32 bit-rot mismatch: expected {:#010x}, calculated {:#010x}",
                            expected_crc, actual_crc
                        ),
                    });
                    return;
                }
            }
            result.verified_ok += 1;
            result.total_bytes_verified += state.size;
            if opts.verbose {
                println!("  [OK] {} (CRC32: {:#010x})", display_path, actual_crc);
            }
        }
        Err(e) => {
            result.corrupted.push(CorruptedFile {
                path: display_path.to_string(),
                reason: format!("I/O error computing CRC32: {}", e),
            });
        }
    }
}

fn state_matches_next(s: &FileState, next_manifest: Option<&SnapshotManifest>) -> bool {
    let Some(next) = next_manifest else {
        return false;
    };
    next.state.iter().any(|ns| {
        ns.path == s.path
            && ns.size == s.size
            && ns.mtime_ns == s.mtime_ns
            && (ns.crc32.is_none() || s.crc32.is_none() || ns.crc32 == s.crc32)
    })
}

fn verify_standard(
    opts: &VerifyOptions,
    dest: &Path,
    result: &mut VerifyResult,
) -> Result<(), String> {
    let hist_dir = dest.join(".version-history");
    let baseline = load_baseline(dest);
    let snapshots = list_snapshots(dest).map_err(|e| e.to_string())?;

    if baseline.is_none() && snapshots.is_empty() {
        return Err(format!(
            "No vcopy backup metadata or snapshots found in '{}'.",
            dest.display()
        ));
    }

    let mut expected_live_paths = HashSet::new();
    if let Some(ref base) = baseline {
        for s in &base.state {
            if is_safe_relative_path(&s.path) {
                expected_live_paths.insert(s.path.clone());
            }
        }
    }

    let mut matched_any_target = false;

    // 1. Verify live mirror files
    let check_live = match opts.target_time.as_deref() {
        None | Some("latest") => true,
        Some(target) => baseline
            .as_ref()
            .is_some_and(|b| !b.snapshot_name.is_empty() && b.snapshot_name.starts_with(target)),
    };

    if check_live {
        if let Some(ref base) = baseline {
            matched_any_target = true;
            println!("\n[Checking Live Mirror: {}]", base.snapshot_name);
            for s in &base.state {
                if !is_safe_relative_path(&s.path) {
                    continue;
                }
                let live_file = dest.join(&s.path);
                let display_str = format!("live/{}", s.path);
                verify_file_state(&live_file, &display_str, s, opts, result);
            }
        }
    }

    // Preload snapshot manifests so reverse-delta displacement can be checked
    let loaded_manifests: Vec<Option<SnapshotManifest>> = snapshots
        .iter()
        .map(|snap| load_manifest(&snap.path).ok())
        .collect();

    // 2. Verify snapshot files
    for (snap_idx, snap) in snapshots.iter().enumerate() {
        if let Some(ref target) = opts.target_time {
            if target == "latest" || !snap.name.starts_with(target) {
                continue;
            }
        }

        matched_any_target = true;

        if let Some(ref m) = loaded_manifests[snap_idx] {
            println!("\n[Checking Snapshot: {}]", snap.name);
            let next_manifest = if snap_idx + 1 < snapshots.len() {
                loaded_manifests[snap_idx + 1].as_ref()
            } else {
                baseline.as_ref()
            };

            let mut expected_snap_files = HashSet::new();

            for s in &m.state {
                if !is_safe_relative_path(&s.path) {
                    continue;
                }
                let snap_file = snap.path.join(&s.path);
                if snap_file.exists() {
                    expected_snap_files.insert(s.path.clone());
                    let display_str = format!(".version-history/{}/{}", snap.name, s.path);
                    verify_file_state(&snap_file, &display_str, s, opts, result);
                } else if opts.target_time.is_some() {
                    // When verifying a specific snapshot in isolation, resolve its backing file
                    // chronologically forward across subsequent snapshots or the live mirror.
                    let mut backing_file = None;
                    for later_snap in &snapshots[snap_idx + 1..] {
                        let cand = later_snap.path.join(&s.path);
                        if cand.is_file() {
                            backing_file = Some(cand);
                            break;
                        }
                    }
                    if backing_file.is_none() {
                        let live_cand = dest.join(&s.path);
                        if live_cand.is_file() {
                            backing_file = Some(live_cand);
                        }
                    }
                    let display_str = format!(".version-history/{}/{}", snap.name, s.path);
                    match backing_file {
                        Some(ref bf) => verify_file_state(bf, &display_str, s, opts, result),
                        None => result.missing.push(display_str),
                    }
                } else if !state_matches_next(s, next_manifest) {
                    // File was modified or deleted in the next transition, so its displaced copy
                    // must reside in this snapshot directory.
                    let display_str = format!(".version-history/{}/{}", snap.name, s.path);
                    result.missing.push(display_str);
                }
            }

            // Check for untracked files inside this snapshot directory
            if opts.target_time.is_none() {
                for entry in WalkDir::new(&snap.path).min_depth(1).into_iter().flatten() {
                    if entry.file_type().is_file() {
                        if let Ok(rel) = entry.path().strip_prefix(&snap.path) {
                            let rel_str = normalize_rel_path(rel);
                            if rel_str == "manifest.json"
                                || rel_str.starts_with(".manifest.json.tmp.")
                                || rel_str.ends_with(".deleted")
                            {
                                continue;
                            }
                            if !expected_snap_files.contains(&rel_str)
                                && is_safe_relative_path(&rel_str)
                            {
                                result
                                    .untracked
                                    .push(format!(".version-history/{}/{}", snap.name, rel_str));
                            }
                        }
                    }
                }
            }
        } else {
            result.corrupted.push(CorruptedFile {
                path: format!(".version-history/{}/manifest.json", snap.name),
                reason: "Manifest missing or unreadable".to_string(),
            });
        }
    }

    if let Some(ref target) = opts.target_time {
        if !matched_any_target {
            return Err(format!(
                "No snapshot found matching target time '{}'.",
                target
            ));
        }
    }

    // 3. Scan for untracked / orphan files in live mirror
    if opts.target_time.is_none() {
        for entry in WalkDir::new(dest)
            .into_iter()
            .filter_entry(|e| {
                if e.depth() == 0 {
                    return true;
                }
                let p = e.path();
                // Skip history and lock files
                !p.starts_with(&hist_dir)
                    && !p
                        .file_name()
                        .is_some_and(|n| is_reserved_flat_name(&n.to_string_lossy()))
            })
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            if let Ok(rel) = entry.path().strip_prefix(dest) {
                let rel_str = normalize_rel_path(rel);
                if !expected_live_paths.contains(&rel_str) && is_safe_relative_path(&rel_str) {
                    result.untracked.push(rel_str);
                }
            }
        }
    }

    Ok(())
}

fn verify_flat(opts: &VerifyOptions, dest: &Path, result: &mut VerifyResult) -> Result<(), String> {
    let flat_manifest = load_flat_manifest(dest).ok_or_else(|| {
        format!(
            "Failed to load flat manifest (.version-history.manifest.jsonl) from {}",
            dest.display()
        )
    })?;

    let mut expected_stored_names = HashSet::new();
    for s in &flat_manifest.baseline.state {
        let stored_name = s.stored_name.as_deref().unwrap_or(&s.path);
        expected_stored_names.insert(stored_name.to_string());
    }
    for snap in &flat_manifest.snapshots {
        for s in &snap.state {
            if let Some(ref arch_name) = s.archive_name {
                expected_stored_names.insert(arch_name.clone());
            }
            if let Some(ref stored_name) = s.stored_name {
                expected_stored_names.insert(stored_name.clone());
            }
        }
    }

    let mut matched_any_target = false;

    // 1. Verify live mirror files
    let check_live = match opts.target_time.as_deref() {
        None | Some("latest") => true,
        Some(target) => {
            !flat_manifest.baseline.snapshot_name.is_empty()
                && flat_manifest.baseline.snapshot_name.starts_with(target)
        }
    };

    if check_live {
        matched_any_target = true;
        println!(
            "\n[Checking Flat Live Mirror: {}]",
            flat_manifest.baseline.snapshot_name
        );
        for s in &flat_manifest.baseline.state {
            let stored_name = s.stored_name.as_deref().unwrap_or(&s.path);
            let file_on_disk = dest.join(stored_name);
            let display_str = format!("live/{}", stored_name);
            verify_file_state(&file_on_disk, &display_str, s, opts, result);
        }
    }

    // 2. Verify snapshot files
    for (snap_idx, snap) in flat_manifest.snapshots.iter().enumerate() {
        if let Some(ref target) = opts.target_time {
            if target == "latest" || !snap.snapshot_name.starts_with(target) {
                continue;
            }
        }

        matched_any_target = true;
        println!("\n[Checking Flat Snapshot: {}]", snap.snapshot_name);
        let next_manifest = if snap_idx + 1 < flat_manifest.snapshots.len() {
            Some(&flat_manifest.snapshots[snap_idx + 1])
        } else {
            Some(&flat_manifest.baseline)
        };

        for s in &snap.state {
            if let Some(ref arch_name) = s.archive_name {
                let file_on_disk = dest.join(arch_name);
                let display_str = format!("archive/{}", arch_name);
                verify_file_state(&file_on_disk, &display_str, s, opts, result);
            } else if opts.target_time.is_some() {
                // Specific snapshot requested: resolve backing file forward across later snapshots or live mirror
                let mut backing_name = None;
                for later_snap in &flat_manifest.snapshots[snap_idx + 1..] {
                    if let Some(st) = later_snap.state.iter().find(|st| st.path == s.path) {
                        if let Some(ref arch) = st.archive_name {
                            backing_name = Some(arch.as_str());
                            break;
                        }
                    }
                }
                let resolved_name = backing_name
                    .unwrap_or_else(|| s.stored_name.as_deref().unwrap_or(s.path.as_str()));
                let file_on_disk = dest.join(resolved_name);
                let display_str = format!("snapshot:{}/{}", snap.snapshot_name, s.path);
                verify_file_state(&file_on_disk, &display_str, s, opts, result);
            } else if !state_matches_next(s, next_manifest) {
                let display_str = format!("snapshot:{}/{}", snap.snapshot_name, s.path);
                result.missing.push(display_str);
            }
        }
    }

    if let Some(ref target) = opts.target_time {
        if !matched_any_target {
            return Err(format!(
                "No snapshot found matching target time '{}'.",
                target
            ));
        }
    }

    // 3. Scan for untracked / orphan files in flat destination
    if opts.target_time.is_none() {
        if let Ok(read_dir) = fs::read_dir(dest) {
            for entry in read_dir.flatten() {
                if let Ok(ft) = entry.file_type() {
                    if ft.is_file() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if is_reserved_flat_name(&name) {
                            continue;
                        }
                        if !expected_stored_names.contains(&name) {
                            result.untracked.push(name);
                        }
                    }
                }
            }
        }
    }

    Ok(())
}
