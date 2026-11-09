use std::ffi::OsStr;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::process;

use chrono::{Duration, Local, NaiveDateTime};

use crate::models::SnapshotManifest;

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

pub fn is_flat_repo(dest: &Path) -> bool {
    dest.join(".vcopy.json").is_file()
}

pub fn load_flat_manifest(dest: &Path) -> Option<crate::models::FlatManifest> {
    let flat_path = dest.join(".vcopy.json");
    if let Ok(file) = File::open(&flat_path) {
        if let Ok(manifest) = serde_json::from_reader(file) {
            return Some(manifest);
        }
    }
    None
}

pub fn save_flat_manifest(dest: &Path, manifest: &crate::models::FlatManifest) -> io::Result<()> {
    fs::create_dir_all(dest)?;
    let target_path = dest.join(".vcopy.json");
    let tmp_path = dest.join(format!(".vcopy.json.tmp.{}", process::id()));

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

pub fn load_manifest(snap_dir: &Path) -> io::Result<SnapshotManifest> {
    let manifest_path = snap_dir.join("manifest.json");
    let file = File::open(manifest_path)?;
    let manifest: SnapshotManifest = serde_json::from_reader(file)?;
    Ok(manifest)
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
    load_last_manifest(dest).map(|(_, m)| m)
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
