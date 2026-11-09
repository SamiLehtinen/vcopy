use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::Path;
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{Datelike, NaiveDateTime};
use walkdir::WalkDir;

use crate::history::{
    is_flat_repo, list_snapshots, load_flat_manifest, load_manifest, safe_remove_snapshot_dir,
    save_flat_manifest,
};
use crate::lock::{acquire_flat_lock, acquire_lock};
use crate::models::SnapshotManifest;
use crate::util::{copy_atomic, is_safe_relative_path, normalize_rel_path, validate_dest_path};

static PRUNE_COPY_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct PruneOptions {
    pub max_versions: Option<usize>,
    pub keep_hourly: usize,
    pub keep_daily: usize,
    pub keep_weekly: usize,
    pub keep_monthly: usize,
    pub keep_yearly: usize,
    pub dry_run: bool,
    pub verbose: bool,
}

pub fn run_prune(opts: &PruneOptions, dest: &Path) -> io::Result<()> {
    validate_dest_path(dest)?;

    if is_flat_repo(dest) {
        return run_prune_flat(opts, dest);
    }

    let hist_dir = dest.join(".version-history");
    if !hist_dir.is_dir() {
        return Ok(());
    }

    let _lock = if !opts.dry_run {
        Some(acquire_lock(dest)?)
    } else {
        None
    };

    let mut snapshots = list_snapshots(dest)?;
    if snapshots.is_empty() {
        return Ok(());
    }

    // Newest first for retention evaluation
    snapshots.sort_by(|a, b| b.name.cmp(&a.name));

    let mut to_keep: HashSet<String> = HashSet::new();

    if let Some(max_versions) = opts.max_versions {
        for s in snapshots.iter().take(max_versions) {
            to_keep.insert(s.name.clone());
        }
    } else {
        // Invariant in GFS mode: always keep the most recent historical snapshot
        if let Some(first) = snapshots.first() {
            to_keep.insert(first.name.clone());
        }

        // Smart GFS retention
        let mut hourly_buckets: HashSet<String> = HashSet::new();
        let mut daily_buckets: HashSet<String> = HashSet::new();
        let mut weekly_buckets: HashSet<String> = HashSet::new();
        let mut monthly_buckets: HashSet<String> = HashSet::new();
        let mut yearly_buckets: HashSet<String> = HashSet::new();

        for s in &snapshots {
            if let Ok(ts) = NaiveDateTime::parse_from_str(&s.name, "%Y-%m-%d_%H%M%S") {
                let h_key = ts.format("%Y-%m-%d_%H").to_string();
                if hourly_buckets.len() < opts.keep_hourly && !hourly_buckets.contains(&h_key) {
                    hourly_buckets.insert(h_key);
                    to_keep.insert(s.name.clone());
                }

                let d_key = ts.format("%Y-%m-%d").to_string();
                if daily_buckets.len() < opts.keep_daily && !daily_buckets.contains(&d_key) {
                    daily_buckets.insert(d_key);
                    to_keep.insert(s.name.clone());
                }

                let iso_week = ts.iso_week();
                let w_key = format!("{}-W{:02}", iso_week.year(), iso_week.week());
                if weekly_buckets.len() < opts.keep_weekly && !weekly_buckets.contains(&w_key) {
                    weekly_buckets.insert(w_key);
                    to_keep.insert(s.name.clone());
                }

                let m_key = ts.format("%Y-%m").to_string();
                if monthly_buckets.len() < opts.keep_monthly && !monthly_buckets.contains(&m_key) {
                    monthly_buckets.insert(m_key);
                    to_keep.insert(s.name.clone());
                }

                let y_key = ts.format("%Y").to_string();
                if yearly_buckets.len() < opts.keep_yearly && !yearly_buckets.contains(&y_key) {
                    yearly_buckets.insert(y_key);
                    to_keep.insert(s.name.clone());
                }
            }
        }
    }

    // Load manifests of all snapshots that are being KEPT, sorted chronologically
    let mut kept_manifests = Vec::new();
    for s in snapshots.iter().filter(|s| to_keep.contains(&s.name)) {
        if let Ok(m) = load_manifest(&s.path) {
            kept_manifests.push((s.clone(), m));
        }
    }
    kept_manifests.sort_by(|a, b| a.0.name.cmp(&b.0.name));

    // Process pruned snapshots in chronological order (oldest to newest)
    let mut to_prune: Vec<_> = snapshots
        .into_iter()
        .filter(|s| !to_keep.contains(&s.name))
        .collect();
    to_prune.sort_by(|a, b| a.name.cmp(&b.name));

    for s in &to_prune {
        if opts.verbose {
            println!("Pruning snapshot: {}", s.name);
        }

        if !opts.dry_run {
            // Find the immediately preceding kept snapshot (if any)
            if let Some((prev_info, prev_manifest)) = kept_manifests
                .iter()
                .rfind(|(k_info, _)| k_info.name.as_str() < s.name.as_str())
            {
                preserve_needed_files(&s.path, &prev_info.path, prev_manifest)?;
            }

            safe_remove_snapshot_dir(&hist_dir, &s.path, &s.name)?;
        }
    }

    Ok(())
}

fn preserve_needed_files(
    prune_dir: &Path,
    prev_kept_dir: &Path,
    prev_manifest: &SnapshotManifest,
) -> io::Result<()> {
    if !prune_dir.is_dir() {
        return Ok(());
    }

    let needed_paths: HashSet<&str> = prev_manifest
        .state
        .iter()
        .filter(|st| is_safe_relative_path(&st.path))
        .map(|st| st.path.as_str())
        .collect();

    let pid = process::id();

    for entry in WalkDir::new(prune_dir).min_depth(1).into_iter().flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        let file_path = entry.path();
        let file_name = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if file_name == "manifest.json" || file_name.ends_with(".deleted") {
            continue;
        }

        let rel = match file_path.strip_prefix(prune_dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let rel_str = normalize_rel_path(rel);
        if !is_safe_relative_path(&rel_str) {
            continue;
        }

        if needed_paths.contains(rel_str.as_str()) {
            let target_path = prev_kept_dir.join(&rel_str);
            if !target_path.exists() {
                if let Some(parent) = target_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                if fs::rename(file_path, &target_path).is_err() {
                    let counter = PRUNE_COPY_COUNTER.fetch_add(1, Ordering::Relaxed);
                    copy_atomic(file_path, &target_path, pid, counter)?;
                }
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Flat Single-Directory Pruning
// ---------------------------------------------------------------------------

fn run_prune_flat(opts: &PruneOptions, dest: &Path) -> io::Result<()> {
    let _lock = if !opts.dry_run {
        Some(acquire_flat_lock(dest)?)
    } else {
        None
    };

    let mut flat_manifest = match load_flat_manifest(dest) {
        Some(f) => f,
        None => return Ok(()),
    };

    if flat_manifest.snapshots.is_empty() {
        return Ok(());
    }

    // Evaluate retention on flat_manifest.snapshots (newest first)
    let mut snap_names: Vec<String> = flat_manifest
        .snapshots
        .iter()
        .map(|s| s.snapshot_name.clone())
        .collect();
    snap_names.sort_by(|a, b| b.cmp(a));

    let mut to_keep: HashSet<String> = HashSet::new();

    if let Some(max_versions) = opts.max_versions {
        for name in snap_names.iter().take(max_versions) {
            to_keep.insert(name.clone());
        }
    } else {
        if let Some(first) = snap_names.first() {
            to_keep.insert(first.clone());
        }

        let mut hourly_buckets: HashSet<String> = HashSet::new();
        let mut daily_buckets: HashSet<String> = HashSet::new();
        let mut weekly_buckets: HashSet<String> = HashSet::new();
        let mut monthly_buckets: HashSet<String> = HashSet::new();
        let mut yearly_buckets: HashSet<String> = HashSet::new();

        for name in &snap_names {
            if let Ok(ts) = NaiveDateTime::parse_from_str(name, "%Y-%m-%d_%H%M%S") {
                let h_key = ts.format("%Y-%m-%d_%H").to_string();
                if hourly_buckets.len() < opts.keep_hourly && !hourly_buckets.contains(&h_key) {
                    hourly_buckets.insert(h_key);
                    to_keep.insert(name.clone());
                }

                let d_key = ts.format("%Y-%m-%d").to_string();
                if daily_buckets.len() < opts.keep_daily && !daily_buckets.contains(&d_key) {
                    daily_buckets.insert(d_key);
                    to_keep.insert(name.clone());
                }

                let iso_week = ts.iso_week();
                let w_key = format!("{}-W{:02}", iso_week.year(), iso_week.week());
                if weekly_buckets.len() < opts.keep_weekly && !weekly_buckets.contains(&w_key) {
                    weekly_buckets.insert(w_key);
                    to_keep.insert(name.clone());
                }

                let m_key = ts.format("%Y-%m").to_string();
                if monthly_buckets.len() < opts.keep_monthly && !monthly_buckets.contains(&m_key) {
                    monthly_buckets.insert(m_key);
                    to_keep.insert(name.clone());
                }

                let y_key = ts.format("%Y").to_string();
                if yearly_buckets.len() < opts.keep_yearly && !yearly_buckets.contains(&y_key) {
                    yearly_buckets.insert(y_key);
                    to_keep.insert(name.clone());
                }
            }
        }
    }

    // Separate kept and pruned snapshots
    let mut kept_snapshots = Vec::new();
    let mut pruned_snapshots = Vec::new();

    for s in flat_manifest.snapshots {
        if to_keep.contains(&s.snapshot_name) {
            kept_snapshots.push(s);
        } else {
            pruned_snapshots.push(s);
        }
    }
    kept_snapshots.sort_by(|a, b| a.snapshot_name.cmp(&b.snapshot_name));
    pruned_snapshots.sort_by(|a, b| a.snapshot_name.cmp(&b.snapshot_name));

    // Consolidate archive_name pointers into immediately preceding kept snapshot (if needed)
    for p_snap in &pruned_snapshots {
        if opts.verbose {
            println!("Pruning flat snapshot: {}", p_snap.snapshot_name);
        }

        // Find immediately preceding kept snapshot
        if let Some(prev_kept) = kept_snapshots
            .iter_mut()
            .rfind(|k| k.snapshot_name.as_str() < p_snap.snapshot_name.as_str())
        {
            for p_state in &p_snap.state {
                if let Some(ref arch_name) = p_state.archive_name {
                    // Check if prev_kept needs this file (i.e. has path with archive_name == None)
                    if let Some(prev_state) = prev_kept
                        .state
                        .iter_mut()
                        .find(|k_st| k_st.path == p_state.path && k_st.archive_name.is_none())
                    {
                        prev_state.archive_name = Some(arch_name.clone());
                    }
                }
            }
        }
    }

    // Collect all archive_names currently referenced across all surviving kept snapshots
    let mut active_archive_files: HashSet<String> = HashSet::new();
    for k in &kept_snapshots {
        for st in &k.state {
            if let Some(ref a) = st.archive_name {
                active_archive_files.insert(a.clone());
            }
        }
    }

    // Any archive_name from pruned_snapshots not in active_archive_files is removed from disk
    if !opts.dry_run {
        for p_snap in &pruned_snapshots {
            for p_state in &p_snap.state {
                if let Some(ref arch_name) = p_state.archive_name {
                    if !active_archive_files.contains(arch_name) {
                        let fpath = dest.join(arch_name);
                        if fpath.is_file() {
                            let _ = fs::remove_file(fpath);
                            if opts.verbose {
                                println!("Removed unreferenced version file: {}", arch_name);
                            }
                        }
                    }
                }
            }
        }

        flat_manifest.snapshots = kept_snapshots;
        save_flat_manifest(dest, &flat_manifest)?;
    }

    Ok(())
}
