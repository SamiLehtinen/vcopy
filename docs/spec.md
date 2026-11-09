# Engineering Specification: `vcopy`

**Version:** 0.1.0  
**Author:** Sami Lehtinen  
**License:** MIT OR Apache-2.0  

---

## 1. Overview & System Invariants

`vcopy` is a zero-dependency, cross-platform CLI backup tool written in pure Rust. It creates directly browseable, uncompressed, link-free filesystem mirrors with a dated historical change record stored in `.version-history/`. It is optimized for non-snapshotting filesystems (exFAT, FAT32, NTFS, ext4, APFS).

### Filesystem & Safety Invariants
* **Zero Links:** Never creates or follows hardlinks, symlinks, or block reflinks. Safe across external USB storage devices and filesystems without link support.
* **Directly Browseable Destination:** The destination mirror root (`<dest>/`) always consists of raw, uncompressed files directly accessible in any standard operating system file manager.
* **Live Baseline & Historical Change Record:** The authoritative state of the current live mirror is persisted in `.version-history/baseline.json`. Whenever a subsequent sync modifies or deletes files, the displaced prior versions and the prior baseline's `manifest.json` are archived into `.version-history/<YYYY-MM-DD_HHMMSS>/`.
* **Deletion Tracking:** Deletions inside historical snapshots are tagged with zero-byte `.deleted` marker files and recorded in `manifest.json`.
* **Nanosecond Timestamps & CRC32 Integrity:** Exact nanosecond source modification timestamps (`mtime_ns`) and fast SIMD-accelerated CRC32 checksums are persisted in `baseline.json` and `manifest.json` for every snapshot.
* **Atomic Staging Writes:** All file copies and JSON manifest writes use temporary staging files (`.<file>.tmp.<pid>.<counter>`) and atomic renames to prevent partial corruption upon interruption.
* **Strict Path & Purge Safety Guards:**
  * Rejects empty paths, filesystem root (`/`), or non-directory targets as backup destination or restore output.
  * Rejects overlapping source and destination paths (`src == dest`, `dest` inside `src`, or `src` inside `dest`) and overlapping restore output paths (`target_out == dest`, `target_out` inside `dest`, or `dest` inside `target_out`).
  * Rejects path collisions when multiple sources map to the same relative destination path within a single sync run.
  * Validates all relative paths loaded from manifests via `is_safe_relative_path` (forbidding absolute paths, `..`, `.`, and `.version-history` segments).
  * Validates snapshot directories via `safe_remove_snapshot_dir` before any `remove_dir_all` call during pruning (requiring strict 17-byte `YYYY-MM-DD_HHMMSS` timestamp format, direct `.version-history` parent containment, and non-symlink directory verification).
* **Non-Blocking Resilient Advisory Locking:** Modifying target paths requires an exclusive advisory lock on `.version-history/.lock`. If the underlying filesystem (e.g., exFAT FUSE) does not support POSIX `flock`/`fcntl`, it automatically falls back to an atomic PID-file locking mechanism.

---

## 2. Directory Layout & Snapshot Structure

### Target Backup Structure
```text
<destination>/
├── file1.txt                           # Live, browseable mirror of file1
├── photos/
│   ├── img01.jpg                       # Live, browseable mirror of img01
│   └── img02.jpg
└── .version-history/
    ├── .lock                           # Advisory concurrency lock
    ├── baseline.json                   # Authoritative manifest of the live mirror
    ├── 2026-11-09_083000/              # Historical snapshot folder (state at 08:30:00)
    │   ├── manifest.json               # Authoritative manifest for 08:30:00
    │   └── file1.txt                   # Displaced version of file1 from 08:30:00
    └── 2026-11-09_090000/              # Historical snapshot folder (state at 09:00:00)
        ├── manifest.json               # Authoritative manifest for 09:00:00
        └── photos/
            ├── img02.jpg               # Displaced version of img02 from 09:00:00
            ├── deleted_pic.jpg         # File from 09:00:00 displaced when deleted later
            └── deleted_pic.jpg.deleted # Zero-byte deletion marker
```

---

## 3. Data Schema & Models

### `SnapshotManifest`
Persisted as pretty-printed JSON in `.version-history/baseline.json` (live mirror) and `.version-history/<YYYY-MM-DD_HHMMSS>/manifest.json` (historical snapshots).

```rust
pub struct SnapshotManifest {
    pub version: u32,
    pub snapshot_name: String,
    pub snapshot_timestamp: DateTime<Utc>,
    pub sources: Vec<SourceMapping>,
    pub summary: RunSummary,
    pub changes: Vec<FileChange>,
    pub state: Vec<FileState>,
}
```

### `FileState`
Represents the authoritative state of a file in the snapshot:
```rust
pub struct FileState {
    pub path: String,
    pub size: u64,
    pub mtime_ns: i64,
    pub crc32: Option<u32>,
    pub mode: Option<u32>,
    pub readonly: Option<bool>,
}
```

### `FileChange`
Represents mutations relative to the prior baseline:
```rust
pub struct FileChange {
    pub path: String,
    pub action: String, // "new", "modified", "deleted", "restored_missing"
    pub prior_size: Option<u64>,
    pub prior_mtime_ns: Option<i64>,
    pub new_size: Option<u64>,
    pub new_mtime_ns: Option<i64>,
}
```

### `RunSummary`
Execution telemetry:
```rust
pub struct RunSummary {
    pub files_copied: usize,
    pub files_modified: usize,
    pub files_deleted: usize,
    pub bytes_transferred: u64,
}
```

---

## 4. Core Algorithms

### 4.1 Sync Engine (`src/sync.rs`)
1. **Path Validation & Lock Acquisition:** Validates source and destination paths via `validate_sync_paths` and acquires `.version-history/.lock`.
2. **Baseline Ingestion:** Loads `baseline.json` (or falls back to the latest snapshot manifest). If no manifest exists and `dest` already contains files, walks `dest` (ignoring `.version-history/`) to establish the initial baseline.
3. **Deletion-First Execution:**
   * **Deleted:** Baseline entries missing from current source paths are processed **before** creations/modifications. The displaced file is moved to `.version-history/<archive_snap>/<path>`, empty parent directories in `dest/` are cleaned up via `cleanup_empty_parents`, and a zero-byte `<path>.deleted` marker is created. Processing deletions first allows seamless file-to-directory and directory-to-file replacements.
   * **New:** Source path not in baseline. File copied to destination via `copy_atomic`.
   * **Modified:** Baseline entry differs in `size`, `mtime_ns`, or `crc32`. Previous version is moved from `dest/<path>` to `.version-history/<archive_snap>/<path>`, and the new version is atomically copied to `dest/<path>`.
   * **Restored Missing:** Baseline entry matches source, but file is missing on `dest` disk. Automatically re-copied (self-healing).
4. **Manifest Persistence:**
   * On an initial sync into an empty destination (`!had_prior_state`), writes `.version-history/baseline.json` without creating an empty historical snapshot directory.
   * On subsequent syncs with changes (`had_prior_state`), writes the archived baseline manifest into `.version-history/<archive_snap>/manifest.json` and updates `.version-history/baseline.json` with the new live mirror state.
   * If zero changes are detected, no files or manifests are written.

### 4.2 Point-in-Time Restore Engine (`src/restore.rs`)
* Validates `dest` and `target_out` via `validate_restore_paths`.
* When restoring `"latest"` (or when `--target-time` matches the live `baseline.json` timestamp), all live mirror files in `dest/` (ignoring `.version-history/`) are atomically copied to `target_out/`.
* When restoring `--target-time <TIMESTAMP>`:
  1. Finds the latest historical snapshot $S_k$ at or prior to `<TIMESTAMP>` and loads $S_k$'s `manifest.json`.
  2. For each file entry in $S_k\text{.manifest.state}$:
     * Searches historical snapshots chronologically starting from $S_k$ ($S_k, S_{k+1}, \dots, S_m$) for the first displaced file at `<state.path>`.
     * If the file was never displaced in $S_k$ or any subsequent snapshot, copies the untouched file from the live destination mirror `dest/<state.path>`.
     * Restores the exact original nanosecond modification timestamp (`state.mtime_ns`) via `set_file_mtime`.

### 4.3 Pruning & Retention Engine (`src/prune.rs`)
* Supports count-based pruning (`-n <N>`) and Grandfather-Father-Son (GFS) retention (`--keep-hourly`, `--keep-daily`, `--keep-weekly`, `--keep-monthly`, `--keep-yearly`).
* **Reverse-Delta Consolidation (`preserve_needed_files`):**
  * Processes pruned snapshots in chronological order (oldest to newest).
  * Before removing a pruned snapshot $S_{\text{prune}}$, locates the immediately preceding kept snapshot $S_{\text{prev\_kept}}$ ($S_{\text{prev\_kept}} < S_{\text{prune}}$).
  * Any displaced file in $S_{\text{prune}}$ whose path belongs to $S_{\text{prev\_kept}}\text{.manifest.state}$ and is not yet present in $S_{\text{prev\_kept}}$ is moved (or atomically copied with preserved `mtime`) into $S_{\text{prev\_kept}}$ before `safe_remove_snapshot_dir` removes $S_{\text{prune}}$.
  * Any I/O error during consolidation aborts pruning before directory removal, preventing data loss.

### 4.4 Resilient Advisory Locking (`src/lock.rs`)
* Attempts non-blocking advisory locking (`try_lock_exclusive`) on `.version-history/.lock`.
* Returns `WouldBlock` error immediately if another process holds the lock.
* If the underlying filesystem returns `ENOTSUP` (95) or `ENOSYS` (38) (common on exFAT/FAT32 FUSE mounts on Linux), falls back to `.version-history/.pid_lock` with atomic file creation and PID liveness validation.

---

## 5. CLI Interface

```text
vcopy [OPTIONS] [SOURCES... DEST]... [COMMAND]

Commands:
  auto     Auto create backup and prune old snapshots (default)
  create   Create a new backup snapshot without pruning
  prune    Prune old snapshots according to retention policy
  restore  Restore files from a specific snapshot or point in time
  list     List all snapshots in destination with summary statistics
```

---

## 6. Test Suite & Verification

The test harness provides 16 end-to-end integration tests:
* `tests/integration_tests.rs`:
  * `test_basic_backup_and_versioning`
  * `test_deletion_and_marker_creation`
  * `test_multiple_sources_mapping`
  * `test_multiple_sources_with_flatten`
  * `test_files_from`
  * `test_concurrency_lock`
  * `test_no_changes_does_not_create_snapshot`
  * `test_dangerous_overlapping_and_root_paths_rejected`
  * `test_file_to_directory_and_directory_to_file_replacement`
  * `test_manifest_path_traversal_ignored`
* `tests/prune_tests.rs`:
  * `test_count_based_pruning`
  * `test_prune_preserves_older_snapshot_restore`
  * `test_prune_ignores_non_snapshot_dirs_and_symlinks`
* `tests/restore_tests.rs`:
  * `test_point_in_time_restore`
  * `test_multi_snapshot_restore_with_later_deletions_and_modifications`
  * `test_restore_latest`
