# Engineering Specification & Implementation Plan: `vcopy`

Zero-dependency, cross-platform CLI backup tool written in pure Rust. Produces directly browseable, uncompressed, link-free filesystem mirrors with a dated change history folder (`.version-history/`) optimized for non-snapshotting filesystems (exFAT, FAT32, NTFS, ext4, APFS).

---

## 1. System Requirements & Architecture

* **Runtime Target:** Single static binary on Linux (`x86_64`, `aarch64` musl), Windows (`x86_64`), and macOS (`x86_64`, `aarch64`).
* **Filesystem Invariants:**
* Must never use hardlinks, symlinks, block reflinks, or alternate streams.
* Destination mirror must remain raw, uncompressed files directly accessible in file managers.
* History is stored in `.version-history/<YYYY-MM-DD_HHMMSS>/`.
* Deletions inside history directories must be marked with zero-byte `.deleted` files.
* Nanosecond source timestamps (`mtime_ns`) and file sizes are authoritatively persisted in a single `manifest.json` per snapshot to bypass exFAT 2-second timestamp drift.


* **Concurrency & Safety:**
* Must acquire an exclusive advisory file lock (`.version-history/.lock`) before modifying target paths.
* Must write new files to temporary filenames (`<path>.tmp.<PID>`) and atomically rename into place.


* **Dependencies:** Pure Rust only (`clap`, `serde`, `serde_json`, `walkdir`, `chrono`, `fs2`). No native C libraries.

---

## 2. Cargo Project Setup

### `Cargo.toml`

```toml
[package]
name = "vcopy"
version = "0.1.0"
edition = "2021"
authors = ["Sami Lehtinen"]
description = "Browseable, zero-link, multi-platform file sync with snapshot history"
license = "MIT OR Apache-2.0"

[dependencies]
clap = { version = "4.5", features = ["derive"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
walkdir = "2.5"
chrono = { version = "0.4", features = ["serde"] }
fs2 = "0.4"

[profile.release]
opt-level = 3
lto = true
codegen-units = 1
panic = "abort"
strip = true

```

---

## 3. Core Data Structures & Models

### `src/models.rs`

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SourceMapping {
    pub source_path: String,
    pub destination_prefix: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FileState {
    pub path: String,
    pub size: u64,
    pub mtime_ns: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FileChange {
    pub path: String,
    pub action: String, // "new", "modified", "deleted"
    pub prior_size: Option<u64>,
    pub prior_mtime_ns: Option<i64>,
    pub new_size: Option<u64>,
    pub new_mtime_ns: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct RunSummary {
    pub files_copied: usize,
    pub files_modified: usize,
    pub files_deleted: usize,
    pub bytes_transferred: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SnapshotManifest {
    pub version: u32,
    pub snapshot_timestamp: DateTime<Utc>,
    pub sources: Vec<SourceMapping>,
    pub summary: RunSummary,
    pub changes: Vec<FileChange>,
    pub state: Vec<FileState>,
}

```

---

## 4. Complete Application Implementation

### `src/main.rs`

```rust
use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process;

use chrono::{DateTime, Datelike, Local, Timelike, Utc};
use clap::{Parser, Subcommand};
use fs2::FileExt;
use walkdir::WalkDir;

mod models;
use models::*;

#[derive(Parser, Debug)]
#[command(name = "vcopy", author, version, about = "Browseable zero-link backup tool with history")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    #[arg(trailing_var_arg = true)]
    args: Vec<PathBuf>,

    #[arg(short = 'f', long = "files-from")]
    files_from: Option<PathBuf>,

    #[arg(short = 'n', long = "max-versions")]
    max_versions: Option<usize>,

    #[arg(long = "keep-hourly", default_value_t = 0)]
    keep_hourly: usize,

    #[arg(long = "keep-daily", default_value_t = 7)]
    keep_daily: usize,

    #[arg(long = "keep-weekly", default_value_t = 4)]
    keep_weekly: usize,

    #[arg(long = "keep-monthly", default_value_t = 12)]
    keep_monthly: usize,

    #[arg(long = "keep-yearly", default_value_t = 1)]
    keep_yearly: usize,

    #[arg(long = "dry-run")]
    dry_run: bool,

    #[arg(short = 'v', long = "verbose")]
    verbose: bool,

    #[arg(long = "flatten")]
    flatten: bool,

    #[arg(long = "target-time")]
    target_time: Option<String>,
}

#[derive(Subcommand, Debug, PartialEq, Eq)]
enum Commands {
    Auto,
    Create,
    Prune,
    Restore,
}

fn normalize_rel_path(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn get_file_metadata(path: &Path) -> std::io::Result<(u64, i64)> {
    let meta = fs::metadata(path)?;
    let size = meta.len();
    let mtime = meta.modified()?;
    let duration = mtime
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let mtime_ns = duration.as_nanos() as i64;
    Ok((size, mtime_ns))
}

fn sanitize_segment(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            other => other,
        })
        .collect()
}

fn acquire_lock(dest: &Path) -> std::io::Result<File> {
    let hist_dir = dest.join(".version-history");
    fs::create_dir_all(&hist_dir)?;
    let lock_path = hist_dir.join(".lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    file.lock_exclusive()?;
    Ok(file)
}

fn parse_sources(sources_in: &[PathBuf], files_from: Option<&PathBuf>) -> Vec<PathBuf> {
    let mut resolved = Vec::new();
    if let Some(ff) = files_from {
        if let Ok(file) = File::open(ff) {
            let reader = BufReader::new(file);
            for line in reader.lines().map_while(Result::ok) {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }
                resolved.push(PathBuf::from(trimmed));
            }
        }
    }
    for s in sources_in {
        resolved.push(s.clone());
    }
    resolved
}

fn build_mappings(sources: &[PathBuf], flatten: bool) -> Vec<SourceMapping> {
    if sources.len() == 1 && !flatten {
        return vec![SourceMapping {
            source_path: sources[0].to_string_lossy().to_string(),
            destination_prefix: "".to_string(),
        }];
    }

    let mut map = Vec::new();
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    for src in sources {
        let base_name = if flatten {
            "".to_string()
        } else {
            let orig = src
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "source".to_string());
            let count = counts.entry(orig.clone()).or_insert(0);
            *count += 1;
            if *count > 1 {
                format!("{}_{}", orig, count)
            } else {
                orig
            }
        };

        map.push(SourceMapping {
            source_path: src.to_string_lossy().to_string(),
            destination_prefix: sanitize_segment(&base_name),
        });
    }
    map
}

fn load_last_manifest(dest: &Path) -> Option<(PathBuf, SnapshotManifest)> {
    let hist_dir = dest.join(".version-history");
    if !hist_dir.is_dir() {
        return None;
    }
    let mut entries = Vec::new();
    if let Ok(read_dir) = fs::read_dir(hist_dir) {
        for entry in read_dir.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                    if name.len() == 17 && name.contains('_') {
                        entries.push(path);
                    }
                }
            }
        }
    }
    entries.sort();
    let last_dir = entries.last()?;
    let manifest_path = last_dir.join("manifest.json");
    if let Ok(file) = File::open(&manifest_path) {
        if let Ok(manifest) = serde_json::from_reader(file) {
            return Some((last_dir.clone(), manifest));
        }
    }
    None
}

fn copy_atomic(src: &Path, dst: &Path, pid: u32) -> std::io::Result<u64> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_name = format!(
        "{}.tmp.{}",
        dst.file_name().unwrap().to_string_lossy(),
        pid
    );
    let tmp_path = dst.with_file_name(tmp_name);

    let bytes = fs::copy(src, &tmp_path)?;
    if let Err(e) = fs::rename(&tmp_path, dst) {
        let _ = fs::remove_file(&tmp_path);
        return Err(e);
    }
    Ok(bytes)
}

fn run_create(cli: &Cli, sources: &[PathBuf], dest: &Path) -> std::io::Result<()> {
    let mappings = build_mappings(sources, cli.flatten);
    let _lock = if !cli.dry_run {
        Some(acquire_lock(dest)?)
    } else {
        None
    };

    let mut baseline_map: BTreeMap<String, FileState> = BTreeMap::new();
    if let Some((_, last_manifest)) = load_last_manifest(dest) {
        for state in last_manifest.state {
            baseline_map.insert(state.path.clone(), state);
        }
    } else if dest.exists() {
        for entry in WalkDir::new(dest).min_depth(1).into_iter().flatten() {
            let path = entry.path();
            if path.starts_with(dest.join(".version-history")) {
                continue;
            }
            if path.is_file() {
                let rel = path.strip_prefix(dest).unwrap();
                let rel_str = normalize_rel_path(rel);
                if let Ok((size, mtime_ns)) = get_file_metadata(path) {
                    baseline_map.insert(
                        rel_str.clone(),
                        FileState {
                            path: rel_str,
                            size,
                            mtime_ns,
                        },
                    );
                }
            }
        }
    }

    let mut current_source_paths: HashSet<String> = HashSet::new();
    let mut changes: Vec<FileChange> = Vec::new();
    let mut new_state: Vec<FileState> = Vec::new();
    let mut summary = RunSummary::default();
    let now = Utc::now();
    let now_local = Local::now();
    let snap_name = now_local.format("%Y-%m-%d_%H%M%S").to_string();
    let snap_dir = dest.join(".version-history").join(&snap_name);
    let pid = process::id();

    for m in &mappings {
        let src_root = Path::new(&m.source_path);
        if !src_root.exists() {
            eprintln!("Warning: source path missing: {}", m.source_path);
            continue;
        }

        if src_root.is_file() {
            let rel_str = if m.destination_prefix.is_empty() {
                sanitize_segment(&src_root.file_name().unwrap().to_string_lossy())
            } else {
                format!(
                    "{}/{}",
                    m.destination_prefix,
                    sanitize_segment(&src_root.file_name().unwrap().to_string_lossy())
                )
            };
            current_source_paths.insert(rel_str.clone());
            if let Ok((size, mtime_ns)) = get_file_metadata(src_root) {
                new_state.push(FileState {
                    path: rel_str.clone(),
                    size,
                    mtime_ns,
                });
                eval_file(
                    src_root,
                    &rel_str,
                    size,
                    mtime_ns,
                    &baseline_map,
                    dest,
                    &snap_dir,
                    cli,
                    pid,
                    &mut changes,
                    &mut summary,
                )?;
            }
            continue;
        }

        for entry in WalkDir::new(src_root).min_depth(1).into_iter().flatten() {
            let path = entry.path();
            if path.is_file() {
                let rel = path.strip_prefix(src_root).unwrap();
                let sanitized_rel = rel
                    .components()
                    .map(|c| sanitize_segment(&c.as_os_str().to_string_lossy()))
                    .collect::<Vec<_>>()
                    .join("/");
                let rel_str = if m.destination_prefix.is_empty() {
                    sanitized_rel
                } else {
                    format!("{}/{}", m.destination_prefix, sanitized_rel)
                };

                current_source_paths.insert(rel_str.clone());
                if let Ok((size, mtime_ns)) = get_file_metadata(path) {
                    new_state.push(FileState {
                        path: rel_str.clone(),
                        size,
                        mtime_ns,
                    });
                    eval_file(
                        path,
                        &rel_str,
                        size,
                        mtime_ns,
                        &baseline_map,
                        dest,
                        &snap_dir,
                        cli,
                        pid,
                        &mut changes,
                        &mut summary,
                    )?;
                }
            }
        }
    }

    // Check for deletions
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

            if !cli.dry_run {
                let live_file = dest.join(old_path);
                let hist_file = snap_dir.join(old_path);
                let deleted_marker = snap_dir.join(format!("{}.deleted", old_path));

                if live_file.exists() {
                    if let Some(parent) = hist_file.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::rename(&live_file, &hist_file)?;
                    File::create(deleted_marker)?;
                }
            }
            if cli.verbose {
                println!("D {}", old_path);
            }
        }
    }

    if changes.is_empty() {
        if cli.verbose {
            println!("No changes detected. Backup is up-to-date.");
        }
        return Ok(());
    }

    if !cli.dry_run {
        let manifest = SnapshotManifest {
            version: 2,
            snapshot_timestamp: now,
            sources: mappings,
            summary,
            changes,
            state: new_state,
        };
        fs::create_dir_all(&snap_dir)?;
        let manifest_file = File::create(snap_dir.join("manifest.json"))?;
        serde_json::to_writer_pretty(manifest_file, &manifest)?;
    }

    Ok(())
}

fn eval_file(
    src_path: &Path,
    rel_str: &str,
    size: u64,
    mtime_ns: i64,
    baseline_map: &BTreeMap<String, FileState>,
    dest: &Path,
    snap_dir: &Path,
    cli: &Cli,
    pid: u32,
    changes: &mut Vec<FileChange>,
    summary: &mut RunSummary,
) -> std::io::Result<()> {
    let dest_file = dest.join(rel_str);
    match baseline_map.get(rel_str) {
        None => {
            // New file
            changes.push(FileChange {
                path: rel_str.to_string(),
                action: "new".to_string(),
                prior_size: None,
                prior_mtime_ns: None,
                new_size: Some(size),
                new_mtime_ns: Some(mtime_ns),
            });
            summary.files_copied += 1;
            if !cli.dry_run {
                let copied = copy_atomic(src_path, &dest_file, pid)?;
                summary.bytes_transferred += copied;
            }
            if cli.verbose {
                println!("A {}", rel_str);
            }
        }
        Some(base) => {
            let time_diff = (base.mtime_ns - mtime_ns).abs();
            let is_modified = base.size != size || time_diff > 2_000_000_000;

            if is_modified {
                changes.push(FileChange {
                    path: rel_str.to_string(),
                    action: "modified".to_string(),
                    prior_size: Some(base.size),
                    prior_mtime_ns: Some(base.mtime_ns),
                    new_size: Some(size),
                    new_mtime_ns: Some(mtime_ns),
                });
                summary.files_modified += 1;

                if !cli.dry_run {
                    if dest_file.exists() {
                        let hist_target = snap_dir.join(rel_str);
                        if let Some(parent) = hist_target.parent() {
                            fs::create_dir_all(parent)?;
                        }
                        fs::rename(&dest_file, &hist_target)?;
                    }
                    let copied = copy_atomic(src_path, &dest_file, pid)?;
                    summary.bytes_transferred += copied;
                }
                if cli.verbose {
                    println!("M {}", rel_str);
                }
            }
        }
    }
    Ok(())
}

fn run_prune(cli: &Cli, dest: &Path) -> std::io::Result<()> {
    let hist_dir = dest.join(".version-history");
    if !hist_dir.is_dir() {
        return Ok(());
    }

    let _lock = if !cli.dry_run {
        Some(acquire_lock(dest)?)
    } else {
        None
    };

    let mut snapshots = Vec::new();
    for entry in fs::read_dir(&hist_dir)?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                if name.len() == 17 && name.contains('_') {
                    snapshots.push((name.to_string(), path));
                }
            }
        }
    }

    snapshots.sort_by(|a, b| b.0.cmp(&a.0)); // Newest first

    let mut to_keep: HashSet<String> = HashSet::new();

    if let Some(max_versions) = cli.max_versions {
        for (name, _) in snapshots.iter().take(max_versions) {
            to_keep.insert(name.clone());
        }
    } else {
        // Smart GFS retention
        let mut hourly_buckets: HashSet<String> = HashSet::new();
        let mut daily_buckets: HashSet<String> = HashSet::new();
        let mut weekly_buckets: HashSet<String> = HashSet::new();
        let mut monthly_buckets: HashSet<String> = HashSet::new();
        let mut yearly_buckets: HashSet<String> = HashSet::new();

        for (name, _) in &snapshots {
            if let Ok(ts) = chrono::NaiveDateTime::parse_from_str(name, "%Y-%m-%d_%H%M%S") {
                let dt: DateTime<Local> =
                    Local::now().timezone().from_local_datetime(&ts).unwrap();

                let h_key = dt.format("%Y-%m-%d_%H").to_string();
                if hourly_buckets.len() < cli.keep_hourly && !hourly_buckets.contains(&h_key) {
                    hourly_buckets.insert(h_key);
                    to_keep.insert(name.clone());
                }

                let d_key = dt.format("%Y-%m-%d").to_string();
                if daily_buckets.len() < cli.keep_daily && !daily_buckets.contains(&d_key) {
                    daily_buckets.insert(d_key);
                    to_keep.insert(name.clone());
                }

                let w_key = format!("{}-W{:02}", dt.year(), dt.iso_week().week());
                if weekly_buckets.len() < cli.keep_weekly && !weekly_buckets.contains(&w_key) {
                    weekly_buckets.insert(w_key);
                    to_keep.insert(name.clone());
                }

                let m_key = dt.format("%Y-%m").to_string();
                if monthly_buckets.len() < cli.keep_monthly && !monthly_buckets.contains(&m_key) {
                    monthly_buckets.insert(m_key);
                    to_keep.insert(name.clone());
                }

                let y_key = dt.format("%Y").to_string();
                if yearly_buckets.len() < cli.keep_yearly && !yearly_buckets.contains(&y_key) {
                    yearly_buckets.insert(y_key);
                    to_keep.insert(name.clone());
                }
            }
        }
    }

    for (name, path) in &snapshots {
        if !to_keep.contains(name) {
            if cli.verbose {
                println!("Pruning snapshot: {}", name);
            }
            if !cli.dry_run {
                fs::remove_dir_all(path)?;
            }
        }
    }

    Ok(())
}

fn run_restore(cli: &Cli, dest: &Path, target_out: &Path) -> std::io::Result<()> {
    let hist_dir = dest.join(".version-history");
    let mut snapshots = Vec::new();

    for entry in fs::read_dir(&hist_dir)?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                if name.len() == 17 && name.contains('_') {
                    snapshots.push((name.to_string(), path));
                }
            }
        }
    }
    snapshots.sort_by(|a, b| a.0.cmp(&b.0)); // Chronological ascending

    let target_time_str = cli.target_time.as_deref().unwrap_or("latest");
    let chosen_snapshot = if target_time_str == "latest" {
        snapshots.last()
    } else {
        snapshots
            .iter()
            .filter(|(name, _)| name.as_str() <= target_time_str)
            .last()
    };

    let (chosen_name, chosen_path) = match chosen_snapshot {
        Some(s) => s,
        None => {
            eprintln!("No matching snapshot found at or prior to target time.");
            process::exit(1);
        }
    };

    println!("Restoring from snapshot state: {}", chosen_name);
    let manifest_file = File::open(chosen_path.join("manifest.json"))?;
    let manifest: SnapshotManifest = serde_json::from_reader(manifest_file)?;

    for state in manifest.state {
        let live_candidate = dest.join(&state.path);
        let out_target = target_out.join(&state.path);
        if let Some(parent) = out_target.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut matched = false;
        if live_candidate.is_file() {
            if let Ok((size, mtime_ns)) = get_file_metadata(&live_candidate) {
                let time_diff = (mtime_ns - state.mtime_ns).abs();
                if size == state.size && time_diff <= 2_000_000_000 {
                    fs::copy(&live_candidate, &out_target)?;
                    matched = true;
                }
            }
        }

        if !matched {
            // Traverse snapshots from target forward to locate historical displaced copy
            let later_snaps = snapshots
                .iter()
                .filter(|(name, _)| name.as_str() > chosen_name.as_str());
            for (_, snap_dir) in later_snaps {
                let hist_candidate = snap_dir.join(&state.path);
                if hist_candidate.is_file() {
                    fs::copy(&hist_candidate, &out_target)?;
                    matched = true;
                    break;
                }
            }
        }

        if !matched && cli.verbose {
            eprintln!("Warning: could not locate raw file for {}", state.path);
        }
    }

    Ok(())
}

fn main() -> std::io::Result<()> {
    let cli = Cli::parse();

    let mut raw_args = cli.args.clone();
    let command = match cli.command {
        Some(cmd) => cmd,
        None => {
            if raw_args.is_empty() && cli.files_from.is_none() {
                eprintln!("Error: destination or sources missing. Run with --help.");
                process::exit(1);
            }
            Commands::Auto
        }
    };

    match command {
        Commands::Auto | Commands::Create => {
            if raw_args.is_empty() {
                eprintln!("Error: Destination directory required.");
                process::exit(1);
            }
            let dest = raw_args.pop().unwrap();
            let sources = parse_sources(&raw_args, cli.files_from.as_ref());
            if sources.is_empty() {
                eprintln!("Error: No sources provided via arguments or --files-from.");
                process::exit(1);
            }

            run_create(&cli, &sources, &dest)?;
            if command == Commands::Auto {
                run_prune(&cli, &dest)?;
            }
        }
        Commands::Prune => {
            if raw_args.is_empty() {
                eprintln!("Error: Destination directory required.");
                process::exit(1);
            }
            let dest = raw_args.pop().unwrap();
            run_prune(&cli, &dest)?;
        }
        Commands::Restore => {
            if raw_args.len() < 2 {
                eprintln!("Usage: vcopy restore [OPTIONS] <DESTINATION> <OUTPUT_DIR>");
                process::exit(1);
            }
            let target_out = raw_args.pop().unwrap();
            let dest = raw_args.pop().unwrap();
            run_restore(&cli, &dest, &target_out)?;
        }
    }

    Ok(())
}

```

---

## 5. Automated Test Suite

### `tests/integration_tests.rs`

```rust
use std::fs::{self, File};
use std::io::Write;
use std::process::Command;

#[test]
fn test_basic_backup_and_versioning() {
    let tmp = std::env::temp_dir().join("vcopy_test_basic");
    let _ = fs::remove_dir_all(&tmp);

    let src = tmp.join("src");
    let dst = tmp.join("dst");
    fs::create_dir_all(&src).unwrap();

    let doc = src.join("test.txt");
    let mut f = File::create(&doc).unwrap();
    writeln!(f, "version 1").unwrap();

    // Run auto backup
    let status = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&src)
        .arg(&dst)
        .status()
        .unwrap();
    assert!(status.success());
    assert!(dst.join("test.txt").exists());

    // Modify file
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let mut f = File::create(&doc).unwrap();
    writeln!(f, "version 2").unwrap();

    let status = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&src)
        .arg(&dst)
        .status()
        .unwrap();
    assert!(status.success());

    // Verify history directory exists and contains version 1
    let hist = dst.join(".version-history");
    assert!(hist.is_dir());
    let mut entries: Vec<_> = fs::read_dir(&hist)
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_dir())
        .collect();
    assert_eq!(entries.len(), 1);

    let hist_file = entries[0].path().join("test.txt");
    assert!(hist_file.exists());
    let content = fs::read_to_string(hist_file).unwrap();
    assert!(content.contains("version 1"));

    // Verify live mirror contains version 2
    let live_content = fs::read_to_string(dst.join("test.txt")).unwrap();
    assert!(live_content.contains("version 2"));
}

#[test]
fn test_count_based_pruning() {
    let tmp = std::env::temp_dir().join("vcopy_test_prune");
    let _ = fs::remove_dir_all(&tmp);

    let src = tmp.join("src");
    let dst = tmp.join("dst");
    fs::create_dir_all(&src).unwrap();
    let doc = src.join("data.bin");

    for i in 1..=4 {
        let mut f = File::create(&doc).unwrap();
        writeln!(f, "run {}", i).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1050));

        let status = Command::new(env!("CARGO_BIN_EXE_vcopy"))
            .arg("-n")
            .arg("2")
            .arg(&src)
            .arg(&dst)
            .status()
            .unwrap();
        assert!(status.success());
    }

    let hist = dst.join(".version-history");
    let count = fs::read_dir(&hist)
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_dir())
        .count();
    assert_eq!(count, 2);
}

```

---

## 6. Multi-Platform GitHub Actions CI/CD

Create `.github/workflows/release.yml`:

```yaml
name: Release

on:
  push:
    tags:
      - 'v*.*.*'

permissions:
  contents: write

jobs:
  build-release:
    name: Build - ${{ matrix.target }}
    runs-on: ${{ matrix.os }}
    strategy:
      fail-fast: false
      matrix:
        include:
          - target: x86_64-unknown-linux-musl
            os: ubuntu-latest
            use_cross: true
            artifact_name: vcopy
            asset_name: vcopy-x86_64-unknown-linux-musl.tar.gz

          - target: aarch64-unknown-linux-musl
            os: ubuntu-latest
            use_cross: true
            artifact_name: vcopy
            asset_name: vcopy-aarch64-unknown-linux-musl.tar.gz

          - target: x86_64-pc-windows-msvc
            os: windows-latest
            use_cross: false
            artifact_name: vcopy.exe
            asset_name: vcopy-x86_64-pc-windows-msvc.zip

          - target: aarch64-apple-darwin
            os: macos-latest
            use_cross: false
            artifact_name: vcopy
            asset_name: vcopy-aarch64-apple-darwin.tar.gz

          - target: x86_64-apple-darwin
            os: macos-13
            use_cross: false
            artifact_name: vcopy
            asset_name: vcopy-x86_64-apple-darwin.tar.gz

    steps:
      - name: Checkout repository
        uses: actions/checkout@v4

      - name: Install Rust toolchain
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}

      - name: Install cross (Linux targets)
        if: matrix.use_cross
        run: cargo install cross --git https://github.com/cross-rs/cross

      - name: Run Tests (Local native only)
        if: "!matrix.use_cross && matrix.os == 'ubuntu-latest'"
        run: cargo test --verbose

      - name: Build binary (native / cargo)
        if: "!matrix.use_cross"
        run: cargo build --release --target ${{ matrix.target }}

      - name: Build binary (cross)
        if: matrix.use_cross
        run: cross build --release --target ${{ matrix.target }}

      - name: Package artifacts (Unix)
        if: matrix.os != 'windows-latest'
        run: |
          tar -czf "${{ matrix.asset_name }}" \
            -C "target/${{ matrix.target }}/release" "${{ matrix.artifact_name }}"

      - name: Package artifacts (Windows)
        if: matrix.os == 'windows-latest'
        shell: pwsh
        run: |
          Compress-Archive -Path "target/${{ matrix.target }}/release/${{ matrix.artifact_name }}" -DestinationPath "${{ matrix.asset_name }}"

      - name: Upload binaries to release
        uses: softprops/action-gh-release@v2
        with:
          files: ${{ matrix.asset_name }}
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}

```

---

## 7. Execution Guide inside Ubuntu Container

Run inside your Ubuntu 24.04 container workspace:

```bash
# 1. Initialize project
cargo new --bin vcopy
cd vcopy

# 2. Write Cargo.toml, src/models.rs, src/main.rs, and tests/integration_tests.rs
# (Paste the source listings provided above)

# 3. Format and check compilation
cargo fmt --check
cargo check

# 4. Run full test suite
cargo test

# 5. Build optimized local binary
cargo build --release

```

