use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{Local, Utc};
use walkdir::WalkDir;

use crate::history::{load_baseline, save_baseline, save_manifest, unique_snapshot_name};
use crate::lock::acquire_lock;
use crate::models::{FileChange, FileState, RunSummary, SnapshotManifest};
use crate::util::{
    build_mappings, cleanup_empty_parents, compute_file_crc32, copy_atomic, get_file_metadata,
    get_file_permissions, is_safe_relative_path, normalize_rel_path, sanitize_segment,
    validate_sync_paths,
};

static COPY_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct SyncOptions {
    pub flatten: bool,
    pub dry_run: bool,
    pub verbose: bool,
    pub preserve_permissions: bool,
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
    validate_sync_paths(sources, dest)?;

    let mappings = build_mappings(sources, opts.flatten);
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

        if src_root.is_file() {
            let file_name = src_root
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "file".to_string());
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

        for entry in WalkDir::new(src_root).min_depth(1).into_iter().flatten() {
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

    let archive_snap_name = if let Some(ref pm) = prior_manifest {
        if !pm.snapshot_name.is_empty() {
            unique_snapshot_name(&hist_dir, &pm.snapshot_name)
        } else {
            unique_snapshot_name(&hist_dir, &raw_now_name)
        }
    } else {
        unique_snapshot_name(&hist_dir, &raw_now_name)
    };

    let snap_dir = hist_dir.join(&archive_snap_name);
    let pid = process::id();

    let mut changes: Vec<FileChange> = Vec::new();
    let mut summary = RunSummary::default();

    // 1. Process deletions first (so file->directory and directory->file replacements succeed)
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
                File::create(deleted_marker)?;
            }
            if opts.verbose {
                println!("D {}", old_path);
            }
        }
    }

    // 2. Process additions, modifications, and self-healing copies
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

    // Ensure the new live baseline snapshot name is distinct and ordered after archive_snap_name
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
                    pm
                }
                None => SnapshotManifest {
                    version: 2,
                    snapshot_name: archive_snap_name.clone(),
                    snapshot_timestamp: now_utc,
                    sources: mappings.clone(),
                    summary: RunSummary::default(),
                    changes: Vec::new(),
                    state: baseline_map.values().cloned().collect(),
                },
            };
            save_manifest(&snap_dir, &archived_manifest)?;
        }

        let new_baseline = SnapshotManifest {
            version: 2,
            snapshot_name: live_snap_name.clone(),
            snapshot_timestamp: now_utc,
            sources: mappings,
            summary,
            changes,
            state: new_state,
        };
        save_baseline(dest, &new_baseline)?;
        Ok(Some(new_baseline))
    } else {
        let dry_manifest = SnapshotManifest {
            version: 2,
            snapshot_name: live_snap_name,
            snapshot_timestamp: now_utc,
            sources: mappings,
            summary,
            changes,
            state: new_state,
        };
        Ok(Some(dry_manifest))
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
            let is_content_modified = base.size != df.size
                || base.mtime_ns != df.mtime_ns
                || (base.crc32.is_some() && df.crc32.is_some() && base.crc32 != df.crc32);
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
