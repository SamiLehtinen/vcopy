use std::collections::HashSet;
use std::fs;
use std::path::Path;
use walkdir::WalkDir;

use crate::history::{
    is_flat_repo, list_snapshots, load_baseline, load_flat_manifest, load_manifest,
};
use crate::models::FileState;
use crate::util::{compute_file_crc32, format_size, is_safe_relative_path, normalize_rel_path};

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

fn verify_standard(
    opts: &VerifyOptions,
    dest: &Path,
    result: &mut VerifyResult,
) -> Result<(), String> {
    let hist_dir = dest.join(".version-history");
    let baseline = load_baseline(dest);
    let snapshots = list_snapshots(dest).map_err(|e| e.to_string())?;

    let mut expected_live_paths = HashSet::new();

    // 1. Verify live mirror files
    if opts.target_time.is_none() || opts.target_time.as_deref() == Some("latest") {
        if let Some(ref base) = baseline {
            println!("\n[Checking Live Mirror: {}]", base.snapshot_name);
            for s in &base.state {
                if !is_safe_relative_path(&s.path) {
                    continue;
                }
                expected_live_paths.insert(s.path.clone());
                let live_file = dest.join(&s.path);
                let display_str = format!("live/{}", s.path);
                verify_file_state(&live_file, &display_str, s, opts, result);
            }
        }
    }

    // 2. Verify snapshot files
    for snap in &snapshots {
        if let Some(ref target) = opts.target_time {
            if target != "latest" && !snap.name.starts_with(target) {
                continue;
            }
        }

        if let Ok(m) = load_manifest(&snap.path) {
            println!("\n[Checking Snapshot: {}]", snap.name);
            for s in &m.state {
                if !is_safe_relative_path(&s.path) {
                    continue;
                }
                let snap_file = snap.path.join(&s.path);
                let display_str = format!(".version-history/{}/{}", snap.name, s.path);
                verify_file_state(&snap_file, &display_str, s, opts, result);
            }
        } else {
            result.corrupted.push(CorruptedFile {
                path: format!(".version-history/{}/manifest.json", snap.name),
                reason: "Manifest missing or unreadable".to_string(),
            });
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
                    && !p.file_name().is_some_and(|n| {
                        let s = n.to_string_lossy();
                        s == ".vcopy.lock" || s == ".vcopy.pid_lock" || s.starts_with(".vcopy.")
                    })
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

    // 1. Verify live mirror files
    if opts.target_time.is_none() || opts.target_time.as_deref() == Some("latest") {
        println!(
            "\n[Checking Flat Live Mirror: {}]",
            flat_manifest.baseline.snapshot_name
        );
        for s in &flat_manifest.baseline.state {
            let stored_name = s.stored_name.as_deref().unwrap_or(&s.path);
            expected_stored_names.insert(stored_name.to_string());
            let file_on_disk = dest.join(stored_name);
            let display_str = format!("live/{}", stored_name);
            verify_file_state(&file_on_disk, &display_str, s, opts, result);
        }
    }

    // 2. Verify snapshot files
    for snap in &flat_manifest.snapshots {
        if let Some(ref target) = opts.target_time {
            if target != "latest" && !snap.snapshot_name.starts_with(target) {
                continue;
            }
        }

        println!("\n[Checking Flat Snapshot: {}]", snap.snapshot_name);
        for s in &snap.state {
            let stored_name = s.stored_name.as_deref().unwrap_or(&s.path);
            expected_stored_names.insert(stored_name.to_string());
            let file_on_disk = dest.join(stored_name);
            let display_str = format!("archive/{}", stored_name);
            verify_file_state(&file_on_disk, &display_str, s, opts, result);
        }
    }

    // 3. Scan for untracked / orphan files in flat destination
    if opts.target_time.is_none() {
        if let Ok(read_dir) = fs::read_dir(dest) {
            for entry in read_dir.flatten() {
                if let Ok(ft) = entry.file_type() {
                    if ft.is_file() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if name.starts_with(".vcopy.")
                            || name.starts_with(".version-history.")
                            || name == ".vcopy.lock"
                            || name == ".vcopy.pid_lock"
                        {
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
