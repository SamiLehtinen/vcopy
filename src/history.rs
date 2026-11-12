use std::collections::{BTreeMap, HashSet};
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process;

use chrono::{DateTime, Duration, Local, NaiveDateTime, Utc};
use walkdir::WalkDir;

use crate::models::{FileChange, FileState, FlatManifest, RunSummary, SnapshotManifest};
use crate::util::{
    compute_file_crc32, get_file_metadata, get_file_permissions, is_reserved_flat_name,
    is_safe_flat_filename, is_safe_relative_path, parse_version_filename,
};

#[derive(Debug, Clone)]
pub struct SnapshotInfo {
    pub name: String,
    pub path: PathBuf,
}

pub fn list_snapshots(dest: &Path) -> io::Result<Vec<SnapshotInfo>> {
    let hist_dir = dest.join(".version-history");
    if !hist_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut snapshots = Vec::new();
    for entry in fs::read_dir(hist_dir)?.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() && !file_type.is_symlink() {
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                if is_valid_snapshot_name(name) {
                    snapshots.push(SnapshotInfo {
                        name: name.to_string(),
                        path,
                    });
                }
            }
        }
    }

    snapshots.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(snapshots)
}

pub fn is_valid_snapshot_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.len() != 17 {
        return false;
    }
    for (idx, &b) in bytes.iter().enumerate() {
        match idx {
            4 | 7 => {
                if b != b'-' {
                    return false;
                }
            }
            10 => {
                if b != b'_' {
                    return false;
                }
            }
            _ => {
                if !b.is_ascii_digit() {
                    return false;
                }
            }
        }
    }
    NaiveDateTime::parse_from_str(name, "%Y-%m-%d_%H%M%S").is_ok()
}

pub fn unique_snapshot_name(hist_dir: &Path, preferred_name: &str) -> String {
    let base_dt = NaiveDateTime::parse_from_str(preferred_name, "%Y-%m-%d_%H%M%S")
        .unwrap_or_else(|_| Local::now().naive_local());

    let mut candidate_dt = base_dt;
    loop {
        let candidate = candidate_dt.format("%Y-%m-%d_%H%M%S").to_string();
        if !hist_dir.join(&candidate).exists() {
            return candidate;
        }
        candidate_dt += Duration::seconds(1);
    }
}

pub fn has_flat_version_files(dest: &Path) -> bool {
    if !dest.is_dir() || dest.join(".version-history").is_dir() {
        return false;
    }
    if let Ok(entries) = fs::read_dir(dest) {
        for entry in entries.flatten() {
            if let Ok(ft) = entry.file_type() {
                if ft.is_file() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if parse_version_filename(&name).is_some() {
                        return true;
                    }
                }
            }
        }
    }
    false
}

pub fn is_flat_repo(dest: &Path) -> bool {
    dest.join(".version-history.baseline.json").is_file()
        || dest.join(".version-history.manifest.jsonl").is_file()
        || dest.join(".versionhistory.baseline.json").is_file()
        || dest.join(".versionhistory.manifest.jsonl").is_file()
        || dest.join(".version-history.json").is_file()
        || dest.join(".versionhistory.json").is_file()
        || dest.join(".vcopy.json").is_file()
        || has_flat_version_files(dest)
}

pub fn load_flat_manifest_from_jsonl(dest: &Path) -> Option<FlatManifest> {
    for name in &[
        ".version-history.manifest.jsonl",
        ".versionhistory.manifest.jsonl",
    ] {
        let jsonl_path = dest.join(name);
        if let Ok(file) = File::open(&jsonl_path) {
            let reader = BufReader::new(file);
            let mut snapshots = Vec::new();
            for line in reader.lines().map_while(Result::ok) {
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    if let Ok(snap) = serde_json::from_str::<SnapshotManifest>(trimmed) {
                        snapshots.push(snap);
                    }
                }
            }
            if !snapshots.is_empty() {
                let baseline = snapshots.pop().unwrap();
                let mut version_counters = BTreeMap::new();
                if let Ok(entries) = fs::read_dir(dest) {
                    for entry in entries.flatten() {
                        if let Ok(ft) = entry.file_type() {
                            if ft.is_file() {
                                let name = entry.file_name().to_string_lossy().to_string();
                                if let Some((base, v)) = parse_version_filename(&name) {
                                    let cur = version_counters.entry(base).or_insert(0);
                                    if v > *cur {
                                        *cur = v;
                                    }
                                }
                            }
                        }
                    }
                }
                for s in &snapshots {
                    for state in &s.state {
                        if let Some(ref arch) = state.archive_name {
                            if let Some((base, v)) = parse_version_filename(arch) {
                                let cur = version_counters.entry(base).or_insert(0);
                                if v > *cur {
                                    *cur = v;
                                }
                            }
                        }
                    }
                }
                let flat = FlatManifest {
                    comment: crate::models::default_manifest_comment(),
                    version: 2,
                    flatten: true,
                    version_counters,
                    snapshots,
                    baseline,
                };
                let _ = save_flat_manifest(dest, &flat);
                return Some(flat);
            }
        }
    }
    None
}

struct VersionedFileInfo {
    base_name: String,
    version: u64,
    file_name: String,
    size: u64,
    mtime_ns: i64,
    crc32: Option<u32>,
    mode: Option<u32>,
    readonly: Option<bool>,
}

pub fn recover_flat_manifest_from_orphans(dest: &Path) -> Option<FlatManifest> {
    if !dest.is_dir() || dest.join(".version-history").is_dir() {
        return None;
    }

    let mut live_files = Vec::new();
    let mut versioned_files: Vec<VersionedFileInfo> = Vec::new();
    let mut version_counters: BTreeMap<String, u64> = BTreeMap::new();

    if let Ok(entries) = fs::read_dir(dest) {
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if !ft.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if is_reserved_flat_name(&name) || !is_safe_flat_filename(&name) {
                continue;
            }
            let Ok((size, mtime_ns, _)) = get_file_metadata(&entry.path()) else {
                continue;
            };
            let (mode, readonly) = get_file_permissions(&entry.path());
            let crc = compute_file_crc32(&entry.path()).ok();

            if let Some((base, v_num)) = parse_version_filename(&name) {
                let cur = version_counters.entry(base.clone()).or_insert(0);
                if v_num > *cur {
                    *cur = v_num;
                }
                versioned_files.push(VersionedFileInfo {
                    base_name: base,
                    version: v_num,
                    file_name: name,
                    size,
                    mtime_ns,
                    crc32: crc,
                    mode,
                    readonly,
                });
            } else {
                live_files.push(FileState {
                    path: name.clone(),
                    stored_name: Some(name),
                    archive_name: None,
                    size,
                    mtime_ns,
                    crc32: crc,
                    mode,
                    readonly,
                });
            }
        }
    }

    if versioned_files.is_empty() {
        return None;
    }

    let mut versions_set = HashSet::new();
    for vf in &versioned_files {
        versions_set.insert(vf.version);
    }
    let mut versions: Vec<u64> = versions_set.into_iter().collect();
    versions.sort();

    let mut snapshots = Vec::new();
    for v in versions {
        let matching: Vec<_> = versioned_files
            .iter()
            .filter(|vf| vf.version == v)
            .collect();
        let max_mtime = matching.iter().map(|vf| vf.mtime_ns).max().unwrap_or(0);
        let dt = if max_mtime > 0 {
            let secs = max_mtime / 1_000_000_000;
            let nsecs = (max_mtime % 1_000_000_000) as u32;
            DateTime::from_timestamp(secs, nsecs).unwrap_or_else(Utc::now)
        } else {
            Utc::now()
        };

        let mut state = Vec::new();
        let mut total_bytes = 0u64;
        let mut changes = Vec::new();

        for vf in &matching {
            total_bytes += vf.size;
            changes.push(FileChange {
                path: vf.base_name.clone(),
                action: "modified".to_string(),
                prior_size: None,
                prior_mtime_ns: None,
                new_size: Some(vf.size),
                new_mtime_ns: Some(vf.mtime_ns),
            });
            state.push(FileState {
                path: vf.base_name.clone(),
                stored_name: Some(vf.base_name.clone()),
                archive_name: Some(vf.file_name.clone()),
                size: vf.size,
                mtime_ns: vf.mtime_ns,
                crc32: vf.crc32,
                mode: vf.mode,
                readonly: vf.readonly,
            });
        }

        for lf in &live_files {
            if lf.mtime_ns <= max_mtime && !state.iter().any(|s| s.path == lf.path) {
                state.push(lf.clone());
            }
        }

        let snap_dt = dt + Duration::seconds(v as i64);
        let snap_name = snap_dt.format("%Y-%m-%d_%H%M%S").to_string();
        snapshots.push(SnapshotManifest {
            comment: crate::models::default_manifest_comment(),
            version: 2,
            snapshot_name: snap_name,
            snapshot_timestamp: snap_dt,
            sources: Vec::new(),
            summary: RunSummary {
                files_copied: 0,
                files_modified: matching.len(),
                files_deleted: 0,
                bytes_transferred: total_bytes,
            },
            changes,
            state,
        });
    }

    let live_total_bytes: u64 = live_files.iter().map(|f| f.size).sum();
    let live_snap_name = Local::now().format("%Y-%m-%d_%H%M%S").to_string();
    let baseline = SnapshotManifest {
        comment: crate::models::default_manifest_comment(),
        version: 2,
        snapshot_name: live_snap_name,
        snapshot_timestamp: Utc::now(),
        sources: Vec::new(),
        summary: RunSummary {
            files_copied: live_files.len(),
            files_modified: 0,
            files_deleted: 0,
            bytes_transferred: live_total_bytes,
        },
        changes: Vec::new(),
        state: live_files,
    };

    let flat = FlatManifest {
        comment: crate::models::default_manifest_comment(),
        version: 2,
        flatten: true,
        version_counters,
        snapshots,
        baseline,
    };

    let _ = save_flat_manifest(dest, &flat);
    Some(flat)
}

pub fn load_flat_manifest(dest: &Path) -> Option<crate::models::FlatManifest> {
    for name in &[
        ".version-history.baseline.json",
        ".versionhistory.baseline.json",
        ".version-history.json",
        ".versionhistory.json",
        ".vcopy.json",
    ] {
        let flat_path = dest.join(name);
        if let Ok(file) = File::open(&flat_path) {
            if let Ok(manifest) = serde_json::from_reader(file) {
                return Some(manifest);
            }
        }
    }
    if let Some(manifest) = load_flat_manifest_from_jsonl(dest) {
        return Some(manifest);
    }
    recover_flat_manifest_from_orphans(dest)
}

pub fn save_flat_manifest(dest: &Path, manifest: &crate::models::FlatManifest) -> io::Result<()> {
    fs::create_dir_all(dest)?;
    let target_path = dest.join(".version-history.baseline.json");
    let tmp_path = dest.join(format!(
        ".version-history.baseline.json.tmp.{}",
        process::id()
    ));

    {
        let file = File::create(&tmp_path)?;
        if let Err(e) = serde_json::to_writer_pretty(&file, manifest) {
            let _ = fs::remove_file(&tmp_path);
            return Err(io::Error::other(e));
        }
        let _ = file.sync_all();
    }

    if let Err(e) = fs::rename(&tmp_path, &target_path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(e);
    }

    // Also write .version-history.manifest.jsonl for the append-only snapshot ledger
    let jsonl_path = dest.join(".version-history.manifest.jsonl");
    let tmp_jsonl = dest.join(format!(
        ".version-history.manifest.jsonl.tmp.{}",
        process::id()
    ));

    {
        let mut file = File::create(&tmp_jsonl)?;
        use std::io::Write;
        for s in &manifest.snapshots {
            let line = serde_json::to_string(s).map_err(io::Error::other)?;
            writeln!(file, "{}", line)?;
        }
        let baseline_line = serde_json::to_string(&manifest.baseline).map_err(io::Error::other)?;
        writeln!(file, "{}", baseline_line)?;
        let _ = file.sync_all();
    }

    if let Err(e) = fs::rename(&tmp_jsonl, &jsonl_path) {
        let _ = fs::remove_file(&tmp_jsonl);
        return Err(e);
    }

    Ok(())
}

fn write_json_atomic(dir: &Path, file_name: &str, manifest: &SnapshotManifest) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let target_path = dir.join(file_name);
    let tmp_path = dir.join(format!(".{}.tmp.{}", file_name, process::id()));

    {
        let file = File::create(&tmp_path)?;
        if let Err(e) = serde_json::to_writer_pretty(&file, manifest) {
            let _ = fs::remove_file(&tmp_path);
            return Err(io::Error::other(e));
        }
        let _ = file.sync_all();
    }

    if let Err(e) = fs::rename(&tmp_path, &target_path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(e);
    }
    Ok(())
}

pub fn recover_snapshot_manifest(snap_dir: &Path, snap_name: &str) -> io::Result<SnapshotManifest> {
    if !snap_dir.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("Snapshot directory does not exist: {}", snap_dir.display()),
        ));
    }

    let mut state = Vec::new();
    let mut changes = Vec::new();
    let mut total_bytes = 0u64;
    let mut deleted_count = 0usize;
    let mut modified_count = 0usize;

    // First collect all .deleted markers
    let mut deleted_paths = HashSet::new();
    for entry in WalkDir::new(snap_dir).min_depth(1).into_iter().flatten() {
        if entry.file_type().is_file() {
            let rel_path = match entry.path().strip_prefix(snap_dir) {
                Ok(p) => p.to_string_lossy().replace('\\', "/"),
                Err(_) => continue,
            };
            if let Some(base) = rel_path.strip_suffix(".deleted") {
                deleted_paths.insert(base.to_string());
            }
        }
    }

    for entry in WalkDir::new(snap_dir).min_depth(1).into_iter().flatten() {
        if entry.file_type().is_file() {
            let rel_path = match entry.path().strip_prefix(snap_dir) {
                Ok(p) => p.to_string_lossy().replace('\\', "/"),
                Err(_) => continue,
            };
            if rel_path == "manifest.json"
                || rel_path.starts_with(".manifest.json.tmp.")
                || rel_path.ends_with(".deleted")
            {
                continue;
            }

            if let Ok((size, mtime_ns, _)) = get_file_metadata(entry.path()) {
                let (mode, readonly) = get_file_permissions(entry.path());
                let crc = compute_file_crc32(entry.path()).ok();
                total_bytes += size;

                let is_deleted = deleted_paths.contains(&rel_path);
                if is_deleted {
                    deleted_count += 1;
                    changes.push(FileChange {
                        path: rel_path.clone(),
                        action: "deleted".to_string(),
                        prior_size: Some(size),
                        prior_mtime_ns: Some(mtime_ns),
                        new_size: None,
                        new_mtime_ns: None,
                    });
                } else {
                    modified_count += 1;
                    changes.push(FileChange {
                        path: rel_path.clone(),
                        action: "modified".to_string(),
                        prior_size: None,
                        prior_mtime_ns: None,
                        new_size: Some(size),
                        new_mtime_ns: Some(mtime_ns),
                    });
                }
                state.push(FileState {
                    path: rel_path,
                    stored_name: None,
                    archive_name: None,
                    size,
                    mtime_ns,
                    crc32: crc,
                    mode,
                    readonly,
                });
            }
        }
    }

    // Handle any deleted marker whose displaced file was empty/absent
    for del in &deleted_paths {
        if !state.iter().any(|s| &s.path == del) {
            deleted_count += 1;
            changes.push(FileChange {
                path: del.clone(),
                action: "deleted".to_string(),
                prior_size: None,
                prior_mtime_ns: None,
                new_size: None,
                new_mtime_ns: None,
            });
        }
    }

    let naive_dt = NaiveDateTime::parse_from_str(snap_name, "%Y-%m-%d_%H%M%S")
        .unwrap_or_else(|_| Local::now().naive_local());
    let timestamp = DateTime::<Utc>::from_naive_utc_and_offset(naive_dt, Utc);

    let manifest = SnapshotManifest {
        comment: crate::models::default_manifest_comment(),
        version: 2,
        snapshot_name: snap_name.to_string(),
        snapshot_timestamp: timestamp,
        sources: Vec::new(),
        summary: RunSummary {
            files_copied: 0,
            files_modified: modified_count,
            files_deleted: deleted_count,
            bytes_transferred: total_bytes,
        },
        changes,
        state,
    };

    let _ = save_manifest(snap_dir, &manifest);
    Ok(manifest)
}

pub fn recover_hierarchical_baseline(dest: &Path) -> Option<SnapshotManifest> {
    let hist_dir = dest.join(".version-history");
    if !hist_dir.is_dir() {
        return None;
    }

    // Try last snapshot if available
    if let Some((_, m)) = load_last_manifest(dest) {
        let _ = save_baseline(dest, &m);
        return Some(m);
    }

    let snapshots = list_snapshots(dest).unwrap_or_default();

    let mut live_states = Vec::new();
    let mut total_bytes = 0u64;

    for entry in WalkDir::new(dest).min_depth(1).into_iter().flatten() {
        let path = entry.path();
        if path.starts_with(&hist_dir) {
            continue;
        }
        if entry.file_type().is_file() {
            let rel_path = match path.strip_prefix(dest) {
                Ok(p) => p.to_string_lossy().replace('\\', "/"),
                Err(_) => continue,
            };
            if !is_safe_relative_path(&rel_path) {
                continue;
            }
            if let Ok((size, mtime_ns, _)) = get_file_metadata(path) {
                let (mode, readonly) = get_file_permissions(path);
                let crc = compute_file_crc32(path).ok();
                total_bytes += size;
                live_states.push(FileState {
                    path: rel_path,
                    stored_name: None,
                    archive_name: None,
                    size,
                    mtime_ns,
                    crc32: crc,
                    mode,
                    readonly,
                });
            }
        }
    }

    if live_states.is_empty() && snapshots.is_empty() {
        return None;
    }

    let snap_name = Local::now().format("%Y-%m-%d_%H%M%S").to_string();
    let manifest = SnapshotManifest {
        comment: crate::models::default_manifest_comment(),
        version: 2,
        snapshot_name: snap_name,
        snapshot_timestamp: Utc::now(),
        sources: Vec::new(),
        summary: RunSummary {
            files_copied: live_states.len(),
            files_modified: 0,
            files_deleted: 0,
            bytes_transferred: total_bytes,
        },
        changes: Vec::new(),
        state: live_states,
    };

    let _ = save_baseline(dest, &manifest);
    Some(manifest)
}

pub fn load_manifest(snap_dir: &Path) -> io::Result<SnapshotManifest> {
    let manifest_path = snap_dir.join("manifest.json");
    if let Ok(file) = File::open(&manifest_path) {
        if let Ok(manifest) = serde_json::from_reader(file) {
            return Ok(manifest);
        }
    }
    if let Some(snap_name) = snap_dir.file_name().and_then(|s| s.to_str()) {
        if is_valid_snapshot_name(snap_name) {
            return recover_snapshot_manifest(snap_dir, snap_name);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("manifest.json not found in '{}'", snap_dir.display()),
    ))
}

pub fn save_manifest(snap_dir: &Path, manifest: &SnapshotManifest) -> io::Result<()> {
    write_json_atomic(snap_dir, "manifest.json", manifest)
}

pub fn load_baseline(dest: &Path) -> Option<SnapshotManifest> {
    let baseline_path = dest.join(".version-history").join("baseline.json");
    if let Ok(file) = File::open(&baseline_path) {
        if let Ok(manifest) = serde_json::from_reader::<_, SnapshotManifest>(file) {
            return Some(manifest);
        }
    }
    if let Some((_, m)) = load_last_manifest(dest) {
        let _ = save_baseline(dest, &m);
        return Some(m);
    }
    recover_hierarchical_baseline(dest)
}

pub fn save_baseline(dest: &Path, manifest: &SnapshotManifest) -> io::Result<()> {
    let hist_dir = dest.join(".version-history");
    write_json_atomic(&hist_dir, "baseline.json", manifest)
}

pub fn load_last_manifest(dest: &Path) -> Option<(PathBuf, SnapshotManifest)> {
    let snapshots = list_snapshots(dest).ok()?;
    let last = snapshots.last()?;
    match load_manifest(&last.path) {
        Ok(m) => Some((last.path.clone(), m)),
        Err(_) => None,
    }
}

pub fn safe_remove_snapshot_dir(
    hist_dir: &Path,
    snap_path: &Path,
    snap_name: &str,
) -> io::Result<()> {
    if !is_valid_snapshot_name(snap_name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "Safety error: refusing to delete invalid snapshot directory name '{}'",
                snap_name
            ),
        ));
    }
    if hist_dir.file_name() != Some(OsStr::new(".version-history")) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "Safety error: snapshot parent '{}' is not .version-history",
                hist_dir.display()
            ),
        ));
    }
    if snap_path.parent() != Some(hist_dir) || snap_path.file_name() != Some(OsStr::new(snap_name))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "Safety error: snapshot path '{}' does not directly reside under '{}'",
                snap_path.display(),
                hist_dir.display()
            ),
        ));
    }

    let meta = fs::symlink_metadata(snap_path)?;
    if meta.file_type().is_symlink() || !meta.file_type().is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "Safety error: snapshot path '{}' is not a regular directory",
                snap_path.display()
            ),
        ));
    }

    fs::remove_dir_all(snap_path)
}
