use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{Local, Utc};
use walkdir::WalkDir;

use crate::history::{
    is_flat_repo, load_baseline, load_flat_manifest, save_baseline, save_flat_manifest,
    save_manifest, unique_snapshot_name,
};
use crate::lock::{acquire_flat_lock, acquire_lock};
use crate::models::{FileChange, FileState, FlatManifest, RunSummary, SnapshotManifest};
use crate::util::{
    build_mappings, cleanup_empty_parents, compute_file_crc32, copy_atomic, get_file_metadata,
    get_file_permissions, is_reserved_flat_name, is_safe_flat_filename, is_safe_relative_path,
    normalize_rel_path, parse_version_filename, sanitize_segment, split_stem_ext,
    validate_sync_paths,
};

static COPY_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct SyncOptions {
    pub flatten: bool,
    pub flat_date: bool,
    pub dry_run: bool,
    pub verbose: bool,
    pub preserve_permissions: bool,
    pub force: bool,
    pub allow_existing: bool,
    pub allow_empty_source: bool,
    pub allow_missing_source: bool,
    pub exclude_filter: crate::exclude::ExcludeFilter,
}

struct DiscoveredFile {
    src_path: PathBuf,
    rel_str: String,
    size: u64,
    mtime_ns: i64,
    crc32: Option<u32>,
    mode: Option<u32>,
    readonly: Option<bool>,
}

pub fn run_create(
    opts: &SyncOptions,
    sources: &[PathBuf],
    dest: &Path,
) -> io::Result<Option<SnapshotManifest>> {
    validate_sync_paths(
        sources,
        dest,
        opts.force || opts.allow_existing,
        opts.force || opts.allow_missing_source,
    )?;

    // Check if destination is already a flat repo or if --flatten requested
    let flat_repo_exists = is_flat_repo(dest);
    let std_repo_exists = dest.join(".version-history").is_dir();

    if opts.flatten && std_repo_exists && !flat_repo_exists {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "Safety error: destination '{}' is already a non-flat repository (contains .version-history/). Cannot mix --flatten with standard mode.",
                dest.display()
            ),
        ));
    }

    let is_flat = opts.flatten || flat_repo_exists;
    if is_flat {
        return run_create_flat(opts, sources, dest);
    }

    run_create_standard(opts, sources, dest)
}

fn run_create_standard(
    opts: &SyncOptions,
    sources: &[PathBuf],
    dest: &Path,
) -> io::Result<Option<SnapshotManifest>> {
    let mappings = build_mappings(sources, false);
    let _lock = if !opts.dry_run {
        Some(acquire_lock(dest)?)
    } else {
        None
    };

    let hist_dir = dest.join(".version-history");
    let prior_manifest = load_baseline(dest);

    let mut baseline_map: BTreeMap<String, FileState> = BTreeMap::new();
    if let Some(ref last_manifest) = prior_manifest {
        for state in &last_manifest.state {
            if is_safe_relative_path(&state.path) {
                baseline_map.insert(state.path.clone(), state.clone());
            }
        }
    } else if dest.exists() {
        // Initial run against a destination that already contains existing files
        for entry in WalkDir::new(dest).min_depth(1).into_iter().flatten() {
            let path = entry.path();
            if path.starts_with(&hist_dir) {
                continue;
            }
            if entry.file_type().is_file() {
                if let Ok(rel) = path.strip_prefix(dest) {
                    let rel_str = normalize_rel_path(rel);
                    if !is_safe_relative_path(&rel_str) {
                        continue;
                    }
                    if let Ok((size, mtime_ns, _)) = get_file_metadata(path) {
                        let (mode, readonly) = if opts.preserve_permissions {
                            get_file_permissions(path)
                        } else {
                            (None, None)
                        };
                        baseline_map.insert(
                            rel_str.clone(),
                            FileState {
                                path: rel_str,
                                stored_name: None,
                                archive_name: None,
                                size,
                                mtime_ns,
                                crc32: compute_file_crc32(path).ok(),
                                mode,
                                readonly,
                            },
                        );
                    }
                }
            }
        }
    }

    let had_prior_state = prior_manifest.is_some() || !baseline_map.is_empty();

    let mut current_source_paths: HashSet<String> = HashSet::new();
    let mut discovered_files: Vec<DiscoveredFile> = Vec::new();
    let mut new_state: Vec<FileState> = Vec::new();

    for m in &mappings {
        let src_root = Path::new(&m.source_path);
        if !src_root.exists() {
            eprintln!("Warning: source path missing: {}", m.source_path);
            continue;
        }

        let mut source_filter = opts.exclude_filter.clone();
        if src_root.is_dir() {
            let ignore_file = src_root.join(".vcopyignore");
            if ignore_file.is_file() {
                let _ = source_filter.add_patterns_from_file(&ignore_file);
            }
        }

        if src_root.is_file() {
            let file_name = src_root
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "file".to_string());
            if source_filter.is_excluded(&file_name, false) {
                continue;
            }
            let rel_str = if m.destination_prefix.is_empty() {
                sanitize_segment(&file_name)
            } else {
                format!("{}/{}", m.destination_prefix, sanitize_segment(&file_name))
            };

            if !is_safe_relative_path(&rel_str) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("Invalid relative destination path: {}", rel_str),
                ));
            }
            if !current_source_paths.insert(rel_str.clone()) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "Destination path collision: multiple source files map to '{}'",
                        rel_str
                    ),
                ));
            }

            if let Ok((size, mtime_ns, _)) = get_file_metadata(src_root) {
                let crc32 = compute_file_crc32(src_root).ok();
                let (mode, readonly) = if opts.preserve_permissions {
                    get_file_permissions(src_root)
                } else {
                    (None, None)
                };
                new_state.push(FileState {
                    path: rel_str.clone(),
                    stored_name: None,
                    archive_name: None,
                    size,
                    mtime_ns,
                    crc32,
                    mode,
                    readonly,
                });
                discovered_files.push(DiscoveredFile {
                    src_path: src_root.to_path_buf(),
                    rel_str,
                    size,
                    mtime_ns,
                    crc32,
                    mode,
                    readonly,
                });
            }
            continue;
        }

        for entry in WalkDir::new(src_root)
            .into_iter()
            .filter_entry(|entry| {
                if entry.depth() == 0 {
                    return true;
                }
                if let Ok(rel) = entry.path().strip_prefix(src_root) {
                    let rel_str = normalize_rel_path(rel);
                    let is_dir = entry.file_type().is_dir();
                    if source_filter.is_excluded(&rel_str, is_dir) {
                        return false;
                    }
                    if is_dir {
                        let sub_ignore = entry.path().join(".vcopyignore");
                        if sub_ignore.is_file() {
                            let _ = source_filter
                                .add_patterns_from_file_with_prefix(&sub_ignore, &rel_str);
                        }
                    }
                    true
                } else {
                    true
                }
            })
            .filter_map(|e| e.ok())
            .filter(|e| e.depth() >= 1)
        {
            let path = entry.path();
            if entry.file_type().is_file() {
                if let Ok(rel) = path.strip_prefix(src_root) {
                    let sanitized_rel = rel
                        .components()
                        .filter_map(|c| match c {
                            std::path::Component::Normal(s) => {
                                Some(sanitize_segment(&s.to_string_lossy()))
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("/");

                    let rel_str = if m.destination_prefix.is_empty() {
                        sanitized_rel
                    } else {
                        format!("{}/{}", m.destination_prefix, sanitized_rel)
                    };

                    if !is_safe_relative_path(&rel_str) {
                        continue;
                    }
                    if !current_source_paths.insert(rel_str.clone()) {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            format!(
                                "Destination path collision: multiple source files map to '{}'",
                                rel_str
                            ),
                        ));
                    }

                    if let Ok((size, mtime_ns, _)) = get_file_metadata(path) {
                        let crc32 = compute_file_crc32(path).ok();
                        let (mode, readonly) = if opts.preserve_permissions {
                            get_file_permissions(path)
                        } else {
                            (None, None)
                        };
                        new_state.push(FileState {
                            path: rel_str.clone(),
                            stored_name: None,
                            archive_name: None,
                            size,
                            mtime_ns,
                            crc32,
                            mode,
                            readonly,
                        });
                        discovered_files.push(DiscoveredFile {
                            src_path: path.to_path_buf(),
                            rel_str,
                            size,
                            mtime_ns,
                            crc32,
                            mode,
                            readonly,
                        });
                    }
                }
            }
        }
    }

    let now_utc = Utc::now();
    let now_local = Local::now();
    let raw_now_name = now_local.format("%Y-%m-%d_%H%M%S").to_string();

    let archive_snap_name = if let Some(ref last_manifest) = prior_manifest {
        if !last_manifest.snapshot_name.is_empty() {
            last_manifest.snapshot_name.clone()
        } else {
            unique_snapshot_name(&hist_dir, &raw_now_name)
        }
    } else {
        unique_snapshot_name(&hist_dir, &raw_now_name)
    };

    let snap_dir = hist_dir.join(&archive_snap_name);
    let pid = process::id();

    // Safety Circuit Breaker: 100% loss / complete wipeout prevention
    if !opts.force
        && !opts.allow_empty_source
        && !baseline_map.is_empty()
        && current_source_paths.is_empty()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "Safety circuit breaker: sync would delete 100% of files in destination (all {} file(s) would be deleted). Aborting sync to prevent complete wipeout. Pass --allow-empty-source or --force if you intentionally want to delete all files.",
                baseline_map.len()
            ),
        ));
    }

    let mut changes: Vec<FileChange> = Vec::new();
    let mut summary = RunSummary::default();

    // 1. Process deletions first
    for (old_path, old_state) in &baseline_map {
        if !is_safe_relative_path(old_path) {
            continue;
        }
        if !current_source_paths.contains(old_path) {
            changes.push(FileChange {
                path: old_path.clone(),
                action: "deleted".to_string(),
                prior_size: Some(old_state.size),
                prior_mtime_ns: Some(old_state.mtime_ns),
                new_size: None,
                new_mtime_ns: None,
            });
            summary.files_deleted += 1;

            if !opts.dry_run {
                let live_file = dest.join(old_path);
                let hist_file = snap_dir.join(old_path);
                let deleted_marker = snap_dir.join(format!("{}.deleted", old_path));

                if live_file.is_file() {
                    if let Some(parent) = hist_file.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::rename(&live_file, &hist_file)?;
                    cleanup_empty_parents(&live_file, dest);
                }

                if let Some(parent) = deleted_marker.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::File::create(deleted_marker)?;
            }
            if opts.verbose {
                println!("D {}", old_path);
            }
        }
    }

    // 2. Process additions and modifications
    let mut eval_ctx = EvalContext {
        baseline_map: &baseline_map,
        dest,
        snap_dir: &snap_dir,
        opts,
        pid,
        changes: &mut changes,
        summary: &mut summary,
    };

    for df in &discovered_files {
        eval_file(df, &mut eval_ctx)?;
    }

    if changes.is_empty() {
        if opts.verbose {
            println!("No changes detected. Backup is up-to-date.");
        }
        return Ok(None);
    }

    let live_snap_name = if had_prior_state && raw_now_name <= archive_snap_name {
        let next_base =
            chrono::NaiveDateTime::parse_from_str(&archive_snap_name, "%Y-%m-%d_%H%M%S")
                .map(|dt| {
                    (dt + chrono::Duration::seconds(1))
                        .format("%Y-%m-%d_%H%M%S")
                        .to_string()
                })
                .unwrap_or_else(|_| raw_now_name.clone());
        unique_snapshot_name(&hist_dir, &next_base)
    } else {
        unique_snapshot_name(&hist_dir, &raw_now_name)
    };

    if !opts.dry_run {
        if had_prior_state {
            let archived_manifest = match prior_manifest {
                Some(mut pm) => {
                    pm.snapshot_name = archive_snap_name.clone();
                    pm.excludes = opts.exclude_filter.raw_patterns().to_vec();
                    pm
                }
                None => SnapshotManifest {
                    comment: crate::models::default_manifest_comment(),
                    version: 2,
                    snapshot_name: archive_snap_name.clone(),
                    snapshot_timestamp: now_utc,
                    sources: mappings.clone(),
                    summary: RunSummary::default(),
                    excludes: opts.exclude_filter.raw_patterns().to_vec(),
                    changes: Vec::new(),
                    state: baseline_map.values().cloned().collect(),
                },
            };
            save_manifest(&snap_dir, &archived_manifest)?;
        }

        let new_baseline = SnapshotManifest {
            comment: crate::models::default_manifest_comment(),
            version: 2,
            snapshot_name: live_snap_name.clone(),
            snapshot_timestamp: now_utc,
            sources: mappings,
            summary: summary.clone(),
            excludes: opts.exclude_filter.raw_patterns().to_vec(),
            changes,
            state: new_state,
        };
        save_baseline(dest, &new_baseline)?;
        if opts.verbose || opts.dry_run {
            let prefix = if opts.dry_run {
                "Dry-run summary"
            } else {
                "Summary"
            };
            println!(
                "{}: {} copied, {} modified, {} deleted, transferred {}",
                prefix,
                summary.files_copied,
                summary.files_modified,
                summary.files_deleted,
                crate::util::format_size(summary.bytes_transferred)
            );
        }
        Ok(Some(new_baseline))
    } else {
        if opts.verbose || opts.dry_run {
            println!(
                "Dry-run summary: {} copied, {} modified, {} deleted, transferred {}",
                summary.files_copied,
                summary.files_modified,
                summary.files_deleted,
                crate::util::format_size(summary.bytes_transferred)
            );
        }
        let dry_manifest = SnapshotManifest {
            comment: crate::models::default_manifest_comment(),
            version: 2,
            snapshot_name: live_snap_name,
            snapshot_timestamp: now_utc,
            sources: mappings,
            summary,
            excludes: opts.exclude_filter.raw_patterns().to_vec(),
            changes,
            state: new_state,
        };
        Ok(Some(dry_manifest))
    }
}

// ---------------------------------------------------------------------------
// Flat Single-Directory Mode Implementation
// ---------------------------------------------------------------------------

fn run_create_flat(
    opts: &SyncOptions,
    sources: &[PathBuf],
    dest: &Path,
) -> io::Result<Option<SnapshotManifest>> {
    let mappings = build_mappings(sources, true);
    let _lock = if !opts.dry_run {
        Some(acquire_flat_lock(dest)?)
    } else {
        None
    };

    let prior_flat_manifest = load_flat_manifest(dest);
    let mut version_counters = prior_flat_manifest
        .as_ref()
        .map(|f| f.version_counters.clone())
        .unwrap_or_default();

    let mut baseline_map: BTreeMap<String, FileState> = BTreeMap::new();
    if let Some(ref flat_manifest) = prior_flat_manifest {
        for state in &flat_manifest.baseline.state {
            if is_safe_relative_path(&state.path)
                && state
                    .stored_name
                    .as_deref()
                    .is_none_or(is_safe_flat_filename)
                && state
                    .archive_name
                    .as_deref()
                    .is_none_or(is_safe_flat_filename)
            {
                baseline_map.insert(state.path.clone(), state.clone());
            }
        }
    } else if dest.exists() {
        // Initial flat scan of destination
        for entry in fs::read_dir(dest)?.flatten() {
            if let Ok(file_type) = entry.file_type() {
                if file_type.is_file() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if is_reserved_flat_name(&name) || !is_safe_flat_filename(&name) {
                        continue;
                    }
                    if let Some((base_name, v_num)) = parse_version_filename(&name) {
                        let cur = version_counters.entry(base_name).or_insert(0);
                        if v_num > *cur {
                            *cur = v_num;
                        }
                        continue;
                    }
                    if let Ok((size, mtime_ns, _)) = get_file_metadata(&entry.path()) {
                        let (mode, readonly) = if opts.preserve_permissions {
                            get_file_permissions(&entry.path())
                        } else {
                            (None, None)
                        };
                        baseline_map.insert(
                            name.clone(),
                            FileState {
                                path: name.clone(),
                                stored_name: Some(name.clone()),
                                archive_name: None,
                                size,
                                mtime_ns,
                                crc32: compute_file_crc32(&entry.path()).ok(),
                                mode,
                                readonly,
                            },
                        );
                    }
                }
            }
        }
    }

    let had_prior_state = prior_flat_manifest.is_some() || !baseline_map.is_empty();

    // Collect all existing files on disk in dest/ to avoid accidental collision
    let mut existing_on_disk: HashSet<String> = HashSet::new();
    if dest.exists() {
        if let Ok(entries) = fs::read_dir(dest) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                existing_on_disk.insert(name);
            }
        }
    }

    let mut current_source_paths: HashSet<String> = HashSet::new();
    let mut raw_discovered: Vec<(DiscoveredFile, String)> = Vec::new(); // (DF, file_name)

    for m in &mappings {
        let src_root = Path::new(&m.source_path);
        if !src_root.exists() {
            eprintln!("Warning: source path missing: {}", m.source_path);
            continue;
        }

        let mut source_filter = opts.exclude_filter.clone();
        if src_root.is_dir() {
            let ignore_file = src_root.join(".vcopyignore");
            if ignore_file.is_file() {
                let _ = source_filter.add_patterns_from_file(&ignore_file);
            }
        }

        if src_root.is_file() {
            let file_name = src_root
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "file".to_string());
            if source_filter.is_excluded(&file_name, false) {
                continue;
            }
            let rel_str = if m.destination_prefix.is_empty() {
                sanitize_segment(&file_name)
            } else {
                format!("{}/{}", m.destination_prefix, sanitize_segment(&file_name))
            };

            if !is_safe_relative_path(&rel_str) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("Invalid relative destination path: {}", rel_str),
                ));
            }
            if !current_source_paths.insert(rel_str.clone()) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("Duplicate source path in scan: '{}'", rel_str),
                ));
            }

            if let Ok((size, mtime_ns, _)) = get_file_metadata(src_root) {
                let crc32 = compute_file_crc32(src_root).ok();
                let (mode, readonly) = if opts.preserve_permissions {
                    get_file_permissions(src_root)
                } else {
                    (None, None)
                };
                raw_discovered.push((
                    DiscoveredFile {
                        src_path: src_root.to_path_buf(),
                        rel_str,
                        size,
                        mtime_ns,
                        crc32,
                        mode,
                        readonly,
                    },
                    file_name,
                ));
            }
            continue;
        }

        for entry in WalkDir::new(src_root)
            .into_iter()
            .filter_entry(|entry| {
                if entry.depth() == 0 {
                    return true;
                }
                if let Ok(rel) = entry.path().strip_prefix(src_root) {
                    let rel_str = normalize_rel_path(rel);
                    let is_dir = entry.file_type().is_dir();
                    if source_filter.is_excluded(&rel_str, is_dir) {
                        return false;
                    }
                    if is_dir {
                        let sub_ignore = entry.path().join(".vcopyignore");
                        if sub_ignore.is_file() {
                            let _ = source_filter
                                .add_patterns_from_file_with_prefix(&sub_ignore, &rel_str);
                        }
                    }
                    true
                } else {
                    true
                }
            })
            .filter_map(|e| e.ok())
            .filter(|e| e.depth() >= 1)
        {
            let path = entry.path();
            if entry.file_type().is_file() {
                if let Ok(rel) = path.strip_prefix(src_root) {
                    let sanitized_rel = rel
                        .components()
                        .filter_map(|c| match c {
                            std::path::Component::Normal(s) => {
                                Some(sanitize_segment(&s.to_string_lossy()))
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("/");

                    let rel_str = if m.destination_prefix.is_empty() {
                        sanitized_rel
                    } else {
                        format!("{}/{}", m.destination_prefix, sanitized_rel)
                    };

                    if !is_safe_relative_path(&rel_str) {
                        continue;
                    }
                    if !current_source_paths.insert(rel_str.clone()) {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            format!("Duplicate source path in scan: '{}'", rel_str),
                        ));
                    }

                    let file_name = path
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| "file".to_string());

                    if let Ok((size, mtime_ns, _)) = get_file_metadata(path) {
                        let crc32 = compute_file_crc32(path).ok();
                        let (mode, readonly) = if opts.preserve_permissions {
                            get_file_permissions(path)
                        } else {
                            (None, None)
                        };
                        raw_discovered.push((
                            DiscoveredFile {
                                src_path: path.to_path_buf(),
                                rel_str,
                                size,
                                mtime_ns,
                                crc32,
                                mode,
                                readonly,
                            },
                            file_name,
                        ));
                    }
                }
            }
        }
    }

    // Set of stored names assigned to active live files in this run.
    // Pre-seed only with SURVIVING baseline stored names so deleted files release their stored_name
    // once archived into a .v<N> file.
    let mut allocated_stored_names: HashSet<String> = HashSet::new();
    for (old_path, st) in &baseline_map {
        if current_source_paths.contains(old_path) {
            if let Some(ref sn) = st.stored_name {
                allocated_stored_names.insert(sn.clone());
            }
        }
    }

    let now_utc = Utc::now();
    let now_local = Local::now();
    let raw_now_name = now_local.format("%Y-%m-%d_%H%M%S").to_string();

    let existing_snapshots = prior_flat_manifest
        .as_ref()
        .map(|f| f.snapshots.as_slice())
        .unwrap_or(&[]);

    let prior_snap_name = prior_flat_manifest
        .as_ref()
        .map(|f| f.baseline.snapshot_name.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| raw_now_name.clone());

    let archive_snap_name = unique_flat_snapshot_name(existing_snapshots, &prior_snap_name, None);
    let pid = process::id();

    // Safety Circuit Breaker: 100% loss / complete wipeout prevention
    if !opts.force
        && !opts.allow_empty_source
        && !baseline_map.is_empty()
        && current_source_paths.is_empty()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "Safety circuit breaker: sync would delete 100% of files in destination (all {} file(s) would be deleted). Aborting sync to prevent complete wipeout. Pass --allow-empty-source or --force if you intentionally want to delete all files.",
                baseline_map.len()
            ),
        ));
    }

    let mut changes: Vec<FileChange> = Vec::new();
    let mut summary = RunSummary::default();

    // Displaced states to archive for the previous baseline snapshot
    let mut archived_states = baseline_map.clone();

    // 1. Process deletions first so their live stored_name is archived to .v<N> and freed
    for (old_path, old_state) in &baseline_map {
        if !current_source_paths.contains(old_path) {
            changes.push(FileChange {
                path: old_path.clone(),
                action: "deleted".to_string(),
                prior_size: Some(old_state.size),
                prior_mtime_ns: Some(old_state.mtime_ns),
                new_size: None,
                new_mtime_ns: None,
            });
            summary.files_deleted += 1;

            let stored = old_state
                .stored_name
                .as_deref()
                .unwrap_or(old_path.as_str());

            if !is_safe_flat_filename(stored) {
                continue;
            }

            let (ver_name, next_ctr) = allocate_version_name(
                stored,
                &version_counters,
                &existing_on_disk,
                &allocated_stored_names,
                opts.flat_date,
                &archive_snap_name,
            );
            version_counters.insert(stored.to_string(), next_ctr);
            existing_on_disk.remove(stored);
            existing_on_disk.insert(ver_name.clone());

            if let Some(arch_st) = archived_states.get_mut(old_path) {
                arch_st.archive_name = Some(ver_name.clone());
            }

            if !opts.dry_run {
                let live_file = dest.join(stored);
                if live_file.is_file() {
                    let hist_file = dest.join(&ver_name);
                    fs::rename(&live_file, &hist_file)?;
                }
            }
            if opts.verbose {
                println!("D {} (archived as {})", old_path, ver_name);
            }
        }
    }

    // Allocate stored_name for discovered files now that deletions have been archived
    let mut discovered_files: Vec<(DiscoveredFile, String)> =
        Vec::with_capacity(raw_discovered.len());
    for (df, file_name) in raw_discovered {
        let stored_name = match baseline_map
            .get(&df.rel_str)
            .and_then(|b| b.stored_name.clone())
            .filter(|s| is_safe_flat_filename(s))
        {
            Some(existing) => existing,
            None => {
                allocate_flat_stored_name(&file_name, &allocated_stored_names, &existing_on_disk)
            }
        };
        allocated_stored_names.insert(stored_name.clone());
        discovered_files.push((df, stored_name));
    }

    // 2. Process additions, modifications, and untouched files
    let mut new_state: Vec<FileState> = Vec::new();

    for (df, stored_name) in &discovered_files {
        let dest_file = dest.join(stored_name);
        let dest_exists = dest_file.is_file();

        match baseline_map.get(&df.rel_str) {
            None => {
                // New file
                changes.push(FileChange {
                    path: df.rel_str.clone(),
                    action: "new".to_string(),
                    prior_size: None,
                    prior_mtime_ns: None,
                    new_size: Some(df.size),
                    new_mtime_ns: Some(df.mtime_ns),
                });
                summary.files_copied += 1;
                existing_on_disk.insert(stored_name.clone());
                if !opts.dry_run {
                    let counter = COPY_COUNTER.fetch_add(1, Ordering::Relaxed);
                    let copied = copy_atomic(&df.src_path, &dest_file, pid, counter)?;
                    summary.bytes_transferred += copied;
                }
                if opts.verbose {
                    println!("A {} -> {}", df.rel_str, stored_name);
                }
            }
            Some(base) => {
                let is_content_modified = is_file_content_modified(base, df);
                let is_perms_modified = opts.preserve_permissions
                    && (base.mode != df.mode || base.readonly != df.readonly);

                if is_content_modified {
                    changes.push(FileChange {
                        path: df.rel_str.clone(),
                        action: "modified".to_string(),
                        prior_size: Some(base.size),
                        prior_mtime_ns: Some(base.mtime_ns),
                        new_size: Some(df.size),
                        new_mtime_ns: Some(df.mtime_ns),
                    });
                    summary.files_modified += 1;

                    let (ver_name, next_ctr) = allocate_version_name(
                        stored_name,
                        &version_counters,
                        &existing_on_disk,
                        &allocated_stored_names,
                        opts.flat_date,
                        &archive_snap_name,
                    );
                    version_counters.insert(stored_name.to_string(), next_ctr);
                    existing_on_disk.insert(ver_name.clone());

                    if let Some(arch_st) = archived_states.get_mut(&df.rel_str) {
                        arch_st.archive_name = Some(ver_name.clone());
                    }

                    if !opts.dry_run {
                        if dest_exists {
                            let hist_file = dest.join(&ver_name);
                            fs::rename(&dest_file, &hist_file)?;
                        }
                        let counter = COPY_COUNTER.fetch_add(1, Ordering::Relaxed);
                        let copied = copy_atomic(&df.src_path, &dest_file, pid, counter)?;
                        summary.bytes_transferred += copied;
                    }
                    if opts.verbose {
                        println!("M {} -> {} (v{})", df.rel_str, stored_name, ver_name);
                    }
                } else if !dest_exists {
                    // Restored missing
                    changes.push(FileChange {
                        path: df.rel_str.clone(),
                        action: "restored_missing".to_string(),
                        prior_size: Some(base.size),
                        prior_mtime_ns: Some(base.mtime_ns),
                        new_size: Some(df.size),
                        new_mtime_ns: Some(df.mtime_ns),
                    });
                    summary.files_copied += 1;
                    existing_on_disk.insert(stored_name.clone());
                    if !opts.dry_run {
                        let counter = COPY_COUNTER.fetch_add(1, Ordering::Relaxed);
                        let copied = copy_atomic(&df.src_path, &dest_file, pid, counter)?;
                        summary.bytes_transferred += copied;
                    }
                    if opts.verbose {
                        println!("+ {} -> {}", df.rel_str, stored_name);
                    }
                } else if is_perms_modified {
                    changes.push(FileChange {
                        path: df.rel_str.clone(),
                        action: "permissions".to_string(),
                        prior_size: Some(base.size),
                        prior_mtime_ns: Some(base.mtime_ns),
                        new_size: Some(df.size),
                        new_mtime_ns: Some(df.mtime_ns),
                    });
                    summary.files_modified += 1;
                    if opts.verbose {
                        println!("P {} -> {}", df.rel_str, stored_name);
                    }
                }
            }
        }

        new_state.push(FileState {
            path: df.rel_str.clone(),
            stored_name: Some(stored_name.clone()),
            archive_name: None,
            size: df.size,
            mtime_ns: df.mtime_ns,
            crc32: df.crc32,
            mode: df.mode,
            readonly: df.readonly,
        });
    }

    if changes.is_empty() {
        if opts.verbose {
            println!("No changes detected. Flat backup is up-to-date.");
        }
        return Ok(None);
    }

    let live_snap_name = unique_flat_snapshot_name(
        existing_snapshots,
        &raw_now_name,
        had_prior_state.then_some(archive_snap_name.as_str()),
    );

    let new_baseline = SnapshotManifest {
        comment: crate::models::default_manifest_comment(),
        version: 2,
        snapshot_name: live_snap_name.clone(),
        snapshot_timestamp: now_utc,
        sources: mappings.clone(),
        summary: summary.clone(),
        excludes: opts.exclude_filter.raw_patterns().to_vec(),
        changes: changes.clone(),
        state: new_state,
    };

    if !opts.dry_run {
        let mut snapshots = prior_flat_manifest
            .as_ref()
            .map(|f| f.snapshots.clone())
            .unwrap_or_default();

        if had_prior_state {
            let archived_manifest = match prior_flat_manifest.as_ref() {
                Some(f) => {
                    let mut pm = f.baseline.clone();
                    pm.snapshot_name = archive_snap_name;
                    pm.excludes = opts.exclude_filter.raw_patterns().to_vec();
                    pm.state = archived_states.into_values().collect();
                    pm
                }
                None => SnapshotManifest {
                    comment: crate::models::default_manifest_comment(),
                    version: 2,
                    snapshot_name: archive_snap_name,
                    snapshot_timestamp: now_utc,
                    sources: mappings.clone(),
                    summary: RunSummary::default(),
                    excludes: opts.exclude_filter.raw_patterns().to_vec(),
                    changes: Vec::new(),
                    state: archived_states.into_values().collect(),
                },
            };
            snapshots.push(archived_manifest);
        }

        let updated_flat_manifest = FlatManifest {
            comment: crate::models::default_manifest_comment(),
            version: 2,
            flatten: true,
            version_counters,
            baseline: new_baseline.clone(),
            snapshots,
        };

        save_flat_manifest(dest, &updated_flat_manifest)?;
    }

    if opts.verbose || opts.dry_run {
        let prefix = if opts.dry_run {
            "Dry-run summary"
        } else {
            "Summary"
        };
        println!(
            "{}: {} copied, {} modified, {} deleted, transferred {}",
            prefix,
            summary.files_copied,
            summary.files_modified,
            summary.files_deleted,
            crate::util::format_size(summary.bytes_transferred)
        );
    }

    Ok(Some(new_baseline))
}

fn unique_flat_snapshot_name(
    existing_snapshots: &[SnapshotManifest],
    preferred_name: &str,
    must_be_after: Option<&str>,
) -> String {
    let mut base_name = preferred_name.to_string();
    if let Some(after) = must_be_after {
        if base_name.as_str() <= after {
            base_name = after.to_string();
        }
    }
    for s in existing_snapshots {
        if base_name.as_str() < s.snapshot_name.as_str() {
            base_name = s.snapshot_name.clone();
        }
    }

    let base_dt = chrono::NaiveDateTime::parse_from_str(&base_name, "%Y-%m-%d_%H%M%S")
        .unwrap_or_else(|_| Local::now().naive_local());

    let mut candidate_dt = base_dt;
    loop {
        let candidate = candidate_dt.format("%Y-%m-%d_%H%M%S").to_string();
        let after_ok = must_be_after.is_none_or(|after| candidate.as_str() > after);
        let unique_ok = existing_snapshots
            .iter()
            .all(|s| s.snapshot_name != candidate);
        if after_ok && unique_ok {
            return candidate;
        }
        candidate_dt += chrono::Duration::seconds(1);
    }
}

fn allocate_flat_stored_name(
    file_name: &str,
    allocated_stored_names: &HashSet<String>,
    existing_on_disk: &HashSet<String>,
) -> String {
    let sanitized = sanitize_segment(file_name);
    let (stem, ext) = split_stem_ext(&sanitized);

    // 1. Try exact name if not reserved and unused
    if !is_reserved_flat_name(&sanitized)
        && !allocated_stored_names.contains(&sanitized)
        && !existing_on_disk.contains(&sanitized)
    {
        return sanitized;
    }

    // 2. Try stem_2.ext, stem_3.ext, ...
    let mut k = 2usize;
    loop {
        let candidate = format!("{}_{}{}", stem, k, ext);
        if !is_reserved_flat_name(&candidate)
            && !allocated_stored_names.contains(&candidate)
            && !existing_on_disk.contains(&candidate)
        {
            return candidate;
        }
        k += 1;
    }
}

fn allocate_version_name(
    stored_name: &str,
    version_counters: &BTreeMap<String, u64>,
    existing_on_disk: &HashSet<String>,
    allocated_stored_names: &HashSet<String>,
    flat_date: bool,
    now_stamp: &str,
) -> (String, u64) {
    let (stem, ext) = split_stem_ext(stored_name);
    let start_ctr = version_counters.get(stored_name).copied().unwrap_or(0) + 1;
    let mut ctr = start_ctr;

    if flat_date {
        let base_candidate = format!("{}.{}{}", stem, now_stamp, ext);
        if !is_reserved_flat_name(&base_candidate)
            && !existing_on_disk.contains(&base_candidate)
            && !allocated_stored_names.contains(&base_candidate)
        {
            return (base_candidate, ctr);
        }
        let mut k = 1;
        loop {
            let candidate = format!("{}.{}.v{}{}", stem, now_stamp, k, ext);
            if !is_reserved_flat_name(&candidate)
                && !existing_on_disk.contains(&candidate)
                && !allocated_stored_names.contains(&candidate)
            {
                return (candidate, ctr);
            }
            k += 1;
        }
    }

    loop {
        let candidate = format!("{}.v{}{}", stem, ctr, ext);
        if !is_reserved_flat_name(&candidate)
            && !existing_on_disk.contains(&candidate)
            && !allocated_stored_names.contains(&candidate)
        {
            return (candidate, ctr);
        }
        ctr += 1;
    }
}

struct EvalContext<'a> {
    baseline_map: &'a BTreeMap<String, FileState>,
    dest: &'a Path,
    snap_dir: &'a Path,
    opts: &'a SyncOptions,
    pid: u32,
    changes: &'a mut Vec<FileChange>,
    summary: &'a mut RunSummary,
}

fn is_file_content_modified(base: &FileState, df: &DiscoveredFile) -> bool {
    if base.size != df.size {
        return true;
    }
    if base.crc32.is_some() && df.crc32.is_some() && base.crc32 != df.crc32 {
        return true;
    }
    if base.mtime_ns != df.mtime_ns {
        // Allow up to 2 seconds (2_000_000_000 ns) tolerance for FAT32/exFAT timestamp rounding
        // when sizes and CRC32 checksums are identical.
        if base.crc32.is_some()
            && base.crc32 == df.crc32
            && base.mtime_ns.abs_diff(df.mtime_ns) <= 2_000_000_000
        {
            return false;
        }
        return true;
    }
    false
}

fn eval_file(df: &DiscoveredFile, ctx: &mut EvalContext) -> io::Result<()> {
    let rel_str = &df.rel_str;
    let dest_file = ctx.dest.join(rel_str);
    let dest_exists = dest_file.is_file();

    match ctx.baseline_map.get(rel_str) {
        None => {
            // Brand new file
            ctx.changes.push(FileChange {
                path: rel_str.to_string(),
                action: "new".to_string(),
                prior_size: None,
                prior_mtime_ns: None,
                new_size: Some(df.size),
                new_mtime_ns: Some(df.mtime_ns),
            });
            ctx.summary.files_copied += 1;
            if !ctx.opts.dry_run {
                let counter = COPY_COUNTER.fetch_add(1, Ordering::Relaxed);
                let copied = copy_atomic(&df.src_path, &dest_file, ctx.pid, counter)?;
                ctx.summary.bytes_transferred += copied;
            }
            if ctx.opts.verbose {
                println!("A {}", rel_str);
            }
        }
        Some(base) => {
            let is_content_modified = is_file_content_modified(base, df);
            let is_perms_modified = ctx.opts.preserve_permissions
                && (base.mode != df.mode || base.readonly != df.readonly);

            if is_content_modified {
                ctx.changes.push(FileChange {
                    path: rel_str.to_string(),
                    action: "modified".to_string(),
                    prior_size: Some(base.size),
                    prior_mtime_ns: Some(base.mtime_ns),
                    new_size: Some(df.size),
                    new_mtime_ns: Some(df.mtime_ns),
                });
                ctx.summary.files_modified += 1;

                if !ctx.opts.dry_run {
                    if dest_exists {
                        let hist_target = ctx.snap_dir.join(rel_str);
                        if let Some(parent) = hist_target.parent() {
                            fs::create_dir_all(parent)?;
                        }
                        fs::rename(&dest_file, &hist_target)?;
                    }
                    let counter = COPY_COUNTER.fetch_add(1, Ordering::Relaxed);
                    let copied = copy_atomic(&df.src_path, &dest_file, ctx.pid, counter)?;
                    ctx.summary.bytes_transferred += copied;
                }
                if ctx.opts.verbose {
                    println!("M {}", rel_str);
                }
            } else if !dest_exists {
                // File was unchanged according to manifest, but missing on destination disk!
                // Self-healing: re-copy to destination
                ctx.changes.push(FileChange {
                    path: rel_str.to_string(),
                    action: "restored_missing".to_string(),
                    prior_size: Some(base.size),
                    prior_mtime_ns: Some(base.mtime_ns),
                    new_size: Some(df.size),
                    new_mtime_ns: Some(df.mtime_ns),
                });
                ctx.summary.files_copied += 1;
                if !ctx.opts.dry_run {
                    let counter = COPY_COUNTER.fetch_add(1, Ordering::Relaxed);
                    let copied = copy_atomic(&df.src_path, &dest_file, ctx.pid, counter)?;
                    ctx.summary.bytes_transferred += copied;
                }
                if ctx.opts.verbose {
                    println!("+ {}", rel_str);
                }
            } else if is_perms_modified {
                // Only file permissions changed (or -p was newly enabled); update manifest metadata
                // without duplicating unchanged file contents in the snapshot directory.
                ctx.changes.push(FileChange {
                    path: rel_str.to_string(),
                    action: "permissions".to_string(),
                    prior_size: Some(base.size),
                    prior_mtime_ns: Some(base.mtime_ns),
                    new_size: Some(df.size),
                    new_mtime_ns: Some(df.mtime_ns),
                });
                ctx.summary.files_modified += 1;
                if ctx.opts.verbose {
                    println!("P {}", rel_str);
                }
            }
        }
    }
    Ok(())
}
