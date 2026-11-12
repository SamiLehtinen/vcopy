use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

use walkdir::WalkDir;

use crate::history::{
    is_flat_repo, list_snapshots, load_baseline, load_flat_manifest, load_manifest, SnapshotInfo,
};
use crate::models::{FileState, FlatManifest};
use crate::util::{
    apply_file_permissions, compute_file_crc32, copy_atomic, get_file_metadata,
    is_reserved_flat_name, is_safe_flat_filename, is_safe_relative_path, mtime_ns_to_system_time,
    set_file_mtime, validate_restore_paths,
};

static RESTORE_COPY_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct RestoreOptions {
    pub target_time: Option<String>,
    pub filter: Option<String>,
    pub verbose: bool,
    pub preserve_permissions: bool,
    pub force: bool,
    pub allow_existing: bool,
}

pub fn matches_restore_filter(rel_path: &str, filter: &str) -> bool {
    let clean_rel = rel_path.trim_matches('/');
    let clean_filt = filter.trim_matches('/');

    if clean_filt.is_empty() {
        return true;
    }

    // 1. Exact match
    if clean_rel == clean_filt {
        return true;
    }

    // 2. Directory prefix match (e.g. filter "docs" matches "docs/guide.md")
    let prefix = format!("{}/", clean_filt);
    if clean_rel.starts_with(&prefix) {
        return true;
    }

    // 3. Glob matching
    if let Ok(pat) = glob::Pattern::new(clean_filt) {
        if pat.matches(clean_rel) {
            return true;
        }
        // If filter is basename only (e.g. "*.pdf"), match basename of rel_path
        if !clean_filt.contains('/') {
            let basename = clean_rel.rsplit('/').next().unwrap_or(clean_rel);
            if pat.matches(basename) {
                return true;
            }
        }
    }

    false
}

pub fn run_restore(opts: &RestoreOptions, dest: &Path, target_out: &Path) -> io::Result<()> {
    validate_restore_paths(dest, target_out, opts.force || opts.allow_existing)?;
    fs::create_dir_all(target_out)?;

    if is_flat_repo(dest) {
        return run_restore_flat(opts, dest, target_out);
    }

    let target_time_str = opts.target_time.as_deref().unwrap_or("latest");

    if target_time_str == "latest" {
        return restore_latest(
            dest,
            target_out,
            opts.filter.as_deref(),
            opts.verbose,
            opts.preserve_permissions,
        );
    }

    let snapshots = list_snapshots(dest)?;

    // If target_time matches the current live baseline snapshot timestamp (and is newer than archived snapshots)
    if let Some(baseline) = load_baseline(dest) {
        let is_newer_than_history = snapshots
            .last()
            .is_none_or(|last| baseline.snapshot_name.as_str() > last.name.as_str());
        if !baseline.snapshot_name.is_empty()
            && is_newer_than_history
            && target_time_str == baseline.snapshot_name.as_str()
        {
            return restore_latest(
                dest,
                target_out,
                opts.filter.as_deref(),
                opts.verbose,
                opts.preserve_permissions,
            );
        }
    }

    if snapshots.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "No snapshots found in destination .version-history",
        ));
    }

    let chosen_idx = snapshots
        .iter()
        .rposition(|s| s.name.as_str() <= target_time_str);

    let chosen_idx = match chosen_idx {
        Some(idx) => idx,
        None => {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "No snapshot found at or prior to target time: {}",
                    target_time_str
                ),
            ));
        }
    };

    let chosen_snapshot = &snapshots[chosen_idx];
    if opts.verbose {
        println!("Restoring from snapshot: {}", chosen_snapshot.name);
    }

    let manifest = load_manifest(&chosen_snapshot.path)?;
    let pid = process::id();
    let mut restored_count = 0usize;

    for state in &manifest.state {
        if !is_safe_relative_path(&state.path) {
            continue;
        }

        if let Some(ref filt) = opts.filter {
            if !matches_restore_filter(&state.path, filt) {
                continue;
            }
        }

        let out_file = target_out.join(&state.path);
        let mut source_to_copy = None;

        // 1. Search from chosen_snapshot forward chronologically for the first displaced copy
        for snap in &snapshots[chosen_idx..] {
            let hist_candidate = snap.path.join(&state.path);
            if let Ok(meta) = fs::symlink_metadata(&hist_candidate) {
                if meta.file_type().is_file() {
                    source_to_copy = Some(hist_candidate);
                    break;
                }
            }
        }

        // 2. If never displaced after chosen_snapshot, use the untouched file in the live mirror
        if source_to_copy.is_none() {
            let live_candidate = dest.join(&state.path);
            if let Ok(meta) = fs::symlink_metadata(&live_candidate) {
                if meta.file_type().is_file() {
                    source_to_copy = Some(live_candidate);
                }
            }
        }

        // 3. Fallback: Orphan scan recovery in hierarchical snapshots and live mirror
        if source_to_copy.is_none() {
            if let Some(recovered) = scan_recover_hierarchical_file(
                dest,
                &snapshots[chosen_idx..],
                state.size,
                state.crc32,
            ) {
                if opts.verbose {
                    println!(
                        "Scan recovered backing file for '{}' -> {}",
                        state.path,
                        recovered.display()
                    );
                }
                source_to_copy = Some(recovered);
            }
        }

        match source_to_copy {
            Some(src_file) => {
                let counter = RESTORE_COPY_COUNTER.fetch_add(1, Ordering::Relaxed);
                copy_atomic(&src_file, &out_file, pid, counter)?;
                let _ = set_file_mtime(&out_file, mtime_ns_to_system_time(state.mtime_ns));
                if opts.preserve_permissions {
                    let _ = apply_file_permissions(&out_file, state.mode, state.readonly);
                }
                restored_count += 1;
                if opts.verbose {
                    println!("Restored: {}", state.path);
                }
            }
            None => {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!(
                        "Restore integrity error: could not locate backing file for '{}' in snapshot {}",
                        state.path, chosen_snapshot.name
                    ),
                ));
            }
        }
    }

    if opts.verbose {
        println!("Restore complete: {} files restored.", restored_count);
    }

    Ok(())
}

fn restore_latest(
    dest: &Path,
    target_out: &Path,
    filter: Option<&str>,
    verbose: bool,
    preserve_permissions: bool,
) -> io::Result<()> {
    let hist_dir = dest.join(".version-history");
    let baseline = load_baseline(dest);
    let baseline_state_map = baseline.as_ref().map(|b| {
        b.state
            .iter()
            .map(|st| (st.path.as_str(), st))
            .collect::<std::collections::HashMap<_, _>>()
    });

    let pid = process::id();
    let mut count = 0usize;

    for entry in WalkDir::new(dest).min_depth(1).into_iter().flatten() {
        let path = entry.path();
        if path.starts_with(&hist_dir) {
            continue;
        }
        if entry.file_type().is_file() {
            if let Ok(rel) = path.strip_prefix(dest) {
                let rel_str = crate::util::normalize_rel_path(rel);
                if let Some(filt) = filter {
                    if !matches_restore_filter(&rel_str, filt) {
                        continue;
                    }
                }
                let out_file = target_out.join(rel);
                let counter = RESTORE_COPY_COUNTER.fetch_add(1, Ordering::Relaxed);
                copy_atomic(path, &out_file, pid, counter)?;

                if let Ok((_, _, mtime)) = get_file_metadata(path) {
                    let _ = set_file_mtime(&out_file, mtime);
                }

                if preserve_permissions {
                    let rel_str = crate::util::normalize_rel_path(rel);
                    if let Some(ref map) = baseline_state_map {
                        if let Some(st) = map.get(rel_str.as_str()) {
                            let _ = apply_file_permissions(&out_file, st.mode, st.readonly);
                        }
                    }
                }

                count += 1;
                if verbose {
                    println!("Restored latest: {}", rel.display());
                }
            }
        }
    }

    if verbose {
        println!("Restore latest complete: {} files restored.", count);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Flat Single-Directory Mode Restore
// ---------------------------------------------------------------------------

fn run_restore_flat(opts: &RestoreOptions, dest: &Path, target_out: &Path) -> io::Result<()> {
    let flat_manifest = load_flat_manifest(dest).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "Could not read .version-history.baseline.json from flat destination",
        )
    })?;

    let target_time_str = opts.target_time.as_deref().unwrap_or("latest");

    if target_time_str == "latest"
        || target_time_str == flat_manifest.baseline.snapshot_name.as_str()
    {
        return restore_flat_manifest(
            &flat_manifest.baseline.state,
            &flat_manifest,
            dest,
            target_out,
            None,
            opts,
        );
    }

    // Find snapshot at or prior to target_time_str
    let chosen_idx = flat_manifest
        .snapshots
        .iter()
        .rposition(|s| s.snapshot_name.as_str() <= target_time_str);

    let chosen_idx = match chosen_idx {
        Some(idx) => idx,
        None => {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "No snapshot found at or prior to target time: {}",
                    target_time_str
                ),
            ));
        }
    };

    let chosen_snap = &flat_manifest.snapshots[chosen_idx];
    if opts.verbose {
        println!(
            "Restoring from flat snapshot: {}",
            chosen_snap.snapshot_name
        );
    }

    restore_flat_manifest(
        &chosen_snap.state,
        &flat_manifest,
        dest,
        target_out,
        Some(chosen_idx),
        opts,
    )
}

fn restore_flat_manifest(
    states: &[FileState],
    flat_manifest: &FlatManifest,
    dest: &Path,
    target_out: &Path,
    snap_idx: Option<usize>,
    opts: &RestoreOptions,
) -> io::Result<()> {
    let pid = process::id();
    let mut restored_count = 0usize;

    for state in states {
        if !is_safe_relative_path(&state.path) {
            continue;
        }

        if let Some(ref filt) = opts.filter {
            let stored = state.stored_name.as_deref().unwrap_or(&state.path);
            if !matches_restore_filter(&state.path, filt) && !matches_restore_filter(stored, filt) {
                continue;
            }
        }

        let out_file = target_out.join(&state.path);
        let mut source_file = None;

        if let Some(idx) = snap_idx {
            // Point in time restore:
            // 1. Check chronologically forward from snap_idx for the FIRST archive_name
            let mut was_displaced = false;
            for s in &flat_manifest.snapshots[idx..] {
                if let Some(st) = s.state.iter().find(|st| st.path == state.path) {
                    if let Some(ref arch_name) = st.archive_name {
                        was_displaced = true;
                        if is_safe_flat_filename(arch_name) {
                            let candidate = dest.join(arch_name);
                            if file_matches_state(&candidate, state) {
                                source_file = Some(candidate);
                            }
                        }
                        break;
                    }
                }
            }

            // 2. If never displaced in subsequent snapshots, check live file
            if !was_displaced && source_file.is_none() {
                let live_name = state.stored_name.as_deref().unwrap_or(state.path.as_str());
                if is_safe_flat_filename(live_name) {
                    let live_cand = dest.join(live_name);
                    if file_matches_state(&live_cand, state) {
                        source_file = Some(live_cand);
                    }
                }
            }
        } else {
            // Latest restore
            let live_name = state.stored_name.as_deref().unwrap_or(state.path.as_str());
            if is_safe_flat_filename(live_name) {
                let live_cand = dest.join(live_name);
                if file_matches_state(&live_cand, state) {
                    source_file = Some(live_cand);
                }
            }
        }

        // 3. Fallback: Scan recovery by (size, crc32) in dest
        if source_file.is_none() {
            if let Some(recovered) = scan_recover_file(dest, state.size, state.crc32) {
                if opts.verbose {
                    println!(
                        "Scan recovered backing file for '{}' -> {}",
                        state.path,
                        recovered.display()
                    );
                }
                source_file = Some(recovered);
            }
        }

        match source_file {
            Some(src) => {
                let counter = RESTORE_COPY_COUNTER.fetch_add(1, Ordering::Relaxed);
                copy_atomic(&src, &out_file, pid, counter)?;
                let _ = set_file_mtime(&out_file, mtime_ns_to_system_time(state.mtime_ns));
                if opts.preserve_permissions {
                    let _ = apply_file_permissions(&out_file, state.mode, state.readonly);
                }
                restored_count += 1;
                if opts.verbose {
                    println!("Restored: {} (from {})", state.path, src.display());
                }
            }
            None => {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!(
                        "Restore integrity error: could not locate backing file for '{}'",
                        state.path
                    ),
                ));
            }
        }
    }

    if opts.verbose {
        println!("Flat restore complete: {} files restored.", restored_count);
    }

    Ok(())
}

fn file_matches_state(path: &Path, state: &FileState) -> bool {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    if !meta.file_type().is_file() || meta.len() != state.size {
        return false;
    }
    if let Some(expected_crc) = state.crc32 {
        compute_file_crc32(path).ok() == Some(expected_crc)
    } else {
        true
    }
}

fn scan_recover_file(
    dest: &Path,
    expected_size: u64,
    expected_crc32: Option<u32>,
) -> Option<PathBuf> {
    let entries = fs::read_dir(dest).ok()?;
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if ft.is_file() {
            let p = entry.path();
            let name = p.file_name()?.to_string_lossy();
            if is_reserved_flat_name(&name) || !is_safe_flat_filename(&name) {
                continue;
            }
            if let Ok(meta) = p.metadata() {
                if meta.len() == expected_size {
                    if let Some(crc) = expected_crc32 {
                        if compute_file_crc32(&p).ok() == Some(crc) {
                            return Some(p);
                        }
                    } else {
                        return Some(p);
                    }
                }
            }
        }
    }
    None
}

fn scan_recover_hierarchical_file(
    dest: &Path,
    active_snapshots: &[SnapshotInfo],
    expected_size: u64,
    expected_crc32: Option<u32>,
) -> Option<PathBuf> {
    for snap in active_snapshots {
        for entry in WalkDir::new(&snap.path).min_depth(1).into_iter().flatten() {
            if entry.file_type().is_file() {
                let p = entry.path();
                let name = p
                    .file_name()
                    .map(|s| s.to_string_lossy())
                    .unwrap_or_default();
                if name == "manifest.json" || name.ends_with(".deleted") {
                    continue;
                }
                if let Ok(meta) = p.metadata() {
                    if meta.len() == expected_size {
                        if let Some(crc) = expected_crc32 {
                            if compute_file_crc32(p).ok() == Some(crc) {
                                return Some(p.to_path_buf());
                            }
                        } else {
                            return Some(p.to_path_buf());
                        }
                    }
                }
            }
        }
    }

    let hist_dir = dest.join(".version-history");
    for entry in WalkDir::new(dest).min_depth(1).into_iter().flatten() {
        let p = entry.path();
        if p.starts_with(&hist_dir) {
            continue;
        }
        if entry.file_type().is_file() {
            let rel = match p.strip_prefix(dest) {
                Ok(r) => r.to_string_lossy(),
                Err(_) => continue,
            };
            if !is_safe_relative_path(&rel) {
                continue;
            }
            if let Ok(meta) = p.metadata() {
                if meta.len() == expected_size {
                    if let Some(crc) = expected_crc32 {
                        if compute_file_crc32(p).ok() == Some(crc) {
                            return Some(p.to_path_buf());
                        }
                    } else {
                        return Some(p.to_path_buf());
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matches_restore_filter() {
        assert!(matches_restore_filter(
            "documents/report.pdf",
            "documents/report.pdf"
        ));
        assert!(matches_restore_filter("documents/report.pdf", "documents"));
        assert!(matches_restore_filter("documents/report.pdf", "documents/"));
        assert!(matches_restore_filter(
            "documents/sub/report.pdf",
            "documents"
        ));
        assert!(!matches_restore_filter("other/report.pdf", "documents"));

        // Glob matching
        assert!(matches_restore_filter("documents/report.pdf", "*.pdf"));
        assert!(!matches_restore_filter("documents/report.txt", "*.pdf"));
        assert!(matches_restore_filter("docs/guide.md", "docs/*.md"));
        assert!(!matches_restore_filter("other/guide.md", "docs/*.md"));
    }
}
