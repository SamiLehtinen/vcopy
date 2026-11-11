# Engineering Specification: `vcopy`

**Version:** 0.2.0  
**Author:** Sami Lehtinen  
**License:** 0BSD  

---

## 1. Overview & System Invariants

`vcopy` is a zero-dependency, cross-platform CLI backup and filesystem synchronization tool written in pure Rust. It creates directly browseable, uncompressed, link-free filesystem mirrors with full historical point-in-time recovery, optimized for non-snapshotting filesystems (exFAT, FAT32, NTFS, ext4, APFS).

`vcopy` supports two primary operational layouts:
1. **Hierarchical Snapshot Mode (Default):** Live browseable mirror in the root destination directory, with displaced previous file revisions archived under dated snapshot directories in `.version-history/<YYYY-MM-DD_HHMMSS>/`.
2. **Single-Directory Flat Mode (`--flatten`):** All files (current live files and displaced historical revisions) reside directly in a single destination folder. Historical versions are stored with monotonic numbered suffixes (`.v0`, `.v1`, ...), source collisions are disambiguated cleanly, and an append-only JSONL catalog alongside heuristic orphan scan recovery provides robust point-in-time recovery without subdirectories.

### Core System Invariants

* **Zero Links:** Never creates or follows hardlinks, symlinks, or block reflinks. Safely works across external USB drives, FAT32, exFAT, network shares, and filesystems without link support.
* **Directly Browseable Destination:** Files in the destination mirror root are always raw, uncompressed files accessible by standard OS file managers (Windows Explorer, macOS Finder, Linux file managers) without proprietary extractors.
* **Nanosecond Timestamps & CRC32 Integrity:** Overcomes exFAT/FAT32 2-second timestamp drift by persisting exact nanosecond modification timestamps (`mtime_ns`) and SIMD-accelerated CRC32 checksums in metadata manifests (`baseline.json`, `manifest.json`, and `.version-history.manifest.jsonl`).
* **Cross-Platform Permission Preservation (`-p / --preserve-permissions`):** Optional tracking of Unix file mode permissions and Windows/DOS readonly attributes. Persisted in snapshot manifests and restored on compatible platforms without encumbering non-POSIX filesystems.
* **Atomic Staging Writes:** All file copies and manifest writes use temporary staging files (`.<file>.tmp.<pid>.<counter>`) and atomic renames to prevent partial corruption from power failures or abrupt termination.
* **Strict Path & Purge Safety Guards:**
  * Rejects empty paths, filesystem root (`/`), or non-directory targets for backup destinations and restore outputs.
  * Rejects overlapping source/destination paths (`src == dest`, `dest` inside `src`, `src` inside `dest`) and restore paths (`target_out == dest`, `target_out` inside `dest`, `dest` inside `target_out`).
  * Validates all relative manifest paths via `is_safe_relative_path` (forbidding absolute paths, `..`, `.`, and `.version-history` path segments).
  * Enforces strict validation before removing snapshot directories during pruning (requiring 17-byte `YYYY-MM-DD_HHMMSS` format, direct parent containment, and non-symlink verification).
* **Non-Blocking Resilient Advisory Locking:** Modifying target paths requires an exclusive advisory lock on `.version-history/.lock`. If the underlying filesystem (e.g., exFAT FUSE) does not support POSIX `flock`/`fcntl`, it automatically falls back to an atomic PID-file locking mechanism (`.version-history/.pid_lock`).

---

## 2. Directory Layouts & Storage Models

### 2.1 Mode 1: Hierarchical Snapshot Mode (Default)

The live files mirror the source layout at `<destination>/`. Displaced historical revisions and metadata are archived in `.version-history/`:

```text
<destination>/
├── file1.txt                           # Current live mirror of file1.txt
├── photos/
│   ├── img01.jpg                       # Current live mirror of img01.jpg
│   └── img02.jpg
└── .version-history/
    ├── .lock                           # Advisory concurrency lock / pid fallback
    ├── baseline.json                   # Authoritative manifest of the current live mirror
    ├── 2026-11-09_083000/              # Historical snapshot folder (state at 08:30:00)
    │   ├── manifest.json               # Full manifest snapshot for 08:30:00
    │   └── file1.txt                   # Displaced version of file1 from 08:30:00
    └── 2026-11-09_090000/              # Historical snapshot folder (state at 09:00:00)
        ├── manifest.json               # Full manifest snapshot for 09:00:00
        └── photos/
            ├── img02.jpg               # Displaced version of img02 from 09:00:00
            ├── deleted_pic.jpg         # File displaced when deleted later
            └── deleted_pic.jpg.deleted # Zero-byte deletion marker
```

### 2.2 Mode 2: Single-Directory Flat Mode (`--flatten`)

All files are stored flat inside `<destination>/` without subfolders. Live files use their base names (with collision disambiguation where needed), and displaced versions receive monotonic version numbers (`.v0`, `.v1`, ...).

```text
<destination>/
├── report.pdf                          # Current live version (report.pdf)
├── report.v0.pdf                       # Oldest displaced revision of report.pdf
├── report.v1.pdf                       # Second displaced revision of report.pdf
├── notes_from_work.txt                 # Disambiguated source collision
├── notes_from_personal.txt             # Disambiguated source collision
├── notes_from_personal.v0.txt          # Displaced revision of notes_from_personal.txt
├── .version-history.lock               # Advisory concurrency lock
├── .version-history.baseline.json      # Latest live state manifest
└── .version-history.manifest.jsonl     # Append-only ledger of all snapshot manifests
```

#### Monotonic Version Naming Convention
- Files with extensions: `<stem>.v<N>.<ext>` (e.g., `document.v0.docx`, `document.v1.docx`).
- Files without extensions: `<name>.v<N>` (e.g., `Makefile.v0`, `Makefile.v1`).

#### Collision Disambiguation Strategy
When multiple sources contribute identical file basenames to the flat destination:
1. First source receives `<stem>_from_<source_basename>.<ext>`.
2. Second source receives `<stem>_from_<source_basename>.<ext>`.
3. If source basenames are identical (e.g. two separate directories named `data`), a collision index is appended: `<stem>_collision_<N>.<ext>`.
4. Live basenames and their historical `.v<N>` variants share the exact same disambiguated stem.

#### Orphan File Scan Recovery
If `.version-history.manifest.jsonl` is damaged, missing, or out of sync:
- `vcopy` scans destination filenames matching the `<base>.v<N>.<ext>` pattern.
- Groups files by logical identity and reconstructs snapshot associations based on disk `mtime_ns` and CRC32 checksums.

---

## 3. Data Schema & Models

Data models are defined in `src/models.rs`.

### `SnapshotManifest`
Authoritative snapshot manifest persisted in `baseline.json`, `<timestamp>/manifest.json`, and as line-delimited records in `manifest.jsonl`:
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
Describes the state of an individual file within a snapshot:
```rust
pub struct FileState {
    pub path: String,                 // Destination-relative file path
    pub size: u64,                    // File size in bytes
    pub mtime_ns: i64,                // Nanosecond source modification timestamp
    pub crc32: Option<u32>,           // SIMD CRC32 hash (optional during fast checks)
    pub mode: Option<u32>,            // Unix file mode bits (when -p enabled)
    pub readonly: Option<bool>,       // Windows/DOS readonly attribute (when -p enabled)
}
```

### `FileChange`
Records change telemetry relative to the preceding baseline:
```rust
pub struct FileChange {
    pub path: String,
    pub action: String,               // "new", "modified", "deleted", "restored_missing"
    pub prior_size: Option<u64>,
    pub prior_mtime_ns: Option<i64>,
    pub new_size: Option<u64>,
    pub new_mtime_ns: Option<i64>,
}
```

### `RunSummary`
Telemetry recorded for each sync run:
```rust
pub struct RunSummary {
    pub files_copied: usize,
    pub files_modified: usize,
    pub files_deleted: usize,
    pub bytes_transferred: u64,
}
```

### `SourceMapping`
```rust
pub struct SourceMapping {
    pub source_path: String,
    pub target_prefix: String,
}
```

---

## 4. Core Algorithms

### 4.1 Sync Engine (`src/sync.rs` & `src/flatten.rs`)

1. **Path Validation & Lock Acquisition:** Validates source and destination paths via `validate_sync_paths` and acquires an exclusive advisory lock on `.version-history/.lock` (with PID fallback).
2. **Baseline Ingestion:** Loads `baseline.json` (or reads the latest record from `manifest.jsonl` in flat mode). If no manifest exists and `dest` contains preexisting files, scans `dest` to establish an initial baseline.
3. **Change Detection:** Computes differences by comparing source file metadata (`size`, `mtime_ns`, and `crc32`) against baseline entries:
   - **New:** File present in source, absent in baseline. Copied to destination via atomic staging.
   - **Modified:** File present in both, but `size`, `mtime_ns`, or `crc32` differs. Prior version is archived, and new version copied via atomic staging.
   - **Deleted:** Baseline entry absent from current source paths. Archived or tagged as deleted.
   - **Restored Missing:** Baseline entry matches source, but file is missing from destination disk. Re-copied atomically (self-healing).
4. **Hierarchical Archiving Execution:**
   - Deleted files in destination are moved to `.version-history/<archive_snap>/<path>` and marked with zero-byte `<path>.deleted`. Empty parent directories in `dest` are cleaned up.
   - Modified files are moved from `dest/<path>` to `.version-history/<archive_snap>/<path>`.
5. **Flat Archiving Execution:**
   - In `--flatten` mode, historical revisions are retained directly in `<destination>/` using the next available monotonic index `<name>.v<next>.<ext>`.
   - Modifying a file moves the existing live `<name>.<ext>` to `<name>.v<next>.<ext>`, then writes the new file as `<name>.<ext>`.
   - Deleted files in flat mode retain their existing `.v<N>` revisions in `<dest>/` while the live `<name>.<ext>` file is removed.
   - Appends the snapshot record to `.version-history/manifest.jsonl`.
6. **Manifest Persistence:**
   - On initial sync into an empty destination, writes `baseline.json` without creating an empty snapshot folder.
   - On subsequent syncs with changes, writes the archived snapshot manifest and updates `baseline.json`.
   - In flat mode, also appends the manifest record to `manifest.jsonl`.
   - If zero changes are detected, no files or manifests are written.

### 4.2 Point-in-Time Restore Engine (`src/restore.rs`)

* **Path Validation:** Rejects overlapping restore destination paths (`target_out == dest`, `target_out` inside `dest`, or `dest` inside `target_out`).
* **Latest Restore:** When restoring `"latest"`, restores the current live mirror files directly to `target_out/` and applies original nanosecond timestamps and permissions (if preserved).
* **Historical Point-in-Time Restore:**
  1. Identifies the closest snapshot $S_k$ at or prior to `--target-time`.
  2. For each file in $S_k\text{.manifest.state}$:
     - **Hierarchical Mode:** Searches historical snapshots chronologically starting from $S_k$ for the first displaced file at `<path>`. If never displaced, reads the untouched file from the live mirror `dest/<path>`.
     - **Flat Mode:** Uses the recorded filename in `manifest.jsonl` or queries the scan recovery engine to resolve the exact version file (`<stem>.v<N>.<ext>` or live `<stem>.<ext>`) matching the snapshot timestamp and CRC32.
  3. Writes files atomically to `target_out/` and sets exact nanosecond modification timestamps via `set_file_mtime`. On Unix platforms, restores file mode bits if `-p` metadata is present.

### 4.3 Pruning & Retention Engine (`src/prune.rs`)

* Supports count-based retention (`-n <N>`) and Grandfather-Father-Son (GFS) retention (`--keep-hourly`, `--keep-daily`, `--keep-weekly`, `--keep-monthly`, `--keep-yearly`).
* **Reverse-Delta Consolidation (`preserve_needed_files`):**
  - Evaluates snapshots chronologically (oldest to newest).
  - Before pruning snapshot $S_{\text{prune}}$, locates the immediately preceding kept snapshot $S_{\text{prev\_kept}}$.
  - Any displaced file in $S_{\text{prune}}$ required by $S_{\text{prev\_kept}}$ and not yet present in $S_{\text{prev\_kept}}$ is migrated into $S_{\text{prev\_kept}}$ before removing $S_{\text{prune}}$.
  - In flat mode, historical version files are consolidated and re-indexed to prevent gaps while preserving point-in-time integrity for all surviving snapshots.
  - Any I/O failure during consolidation aborts pruning before any directory removal.

### 4.4 Resilient Advisory Locking (`src/lock.rs`)

* Attempts non-blocking advisory locking (`try_lock_exclusive`) on `.version-history/.lock`.
* Returns `WouldBlock` error immediately if another process holds the lock.
* Falls back to `.version-history/.pid_lock` with atomic file creation and PID liveness validation if the filesystem returns `ENOTSUP` (95) or `ENOSYS` (38) (common on exFAT/FAT32 FUSE mounts).

---

## 5. CLI Interface Specification

```text
vcopy [OPTIONS] [SOURCES... DEST]... [COMMAND]

Commands:
  auto     Auto create backup and prune old snapshots (default)
  create   Create a new backup snapshot without pruning
  prune    Prune old snapshots according to retention policy
  restore  Restore files from a specific snapshot or point in time
  list     List all snapshots in destination with summary statistics
  manual   Display the comprehensive user manual (plain text or markdown)

Options:
  -c, --config <FILE>            Configuration file (CLI flags, key-value, TOML, or JSON)
  -s, --sources <SOURCE>         Explicit source paths (can be specified multiple times or comma-separated)
  -d, --dest <DEST>              Destination directory (overrides positional destination or config)
  -f, --paths-from <PATTERN>     Read source paths from a file, glob pattern, or stdin ('-') (one per line)
                                 Can be specified multiple times or comma-separated
                                 (aliases: --files-from, --sources-from)
      --flatten                  Single-directory flat mode (all files in destination root)
  -p, --preserve-permissions     Record and restore Unix mode and Windows readonly permissions
  -n, --max-versions <N>         Retain only the N newest snapshots (overrides GFS)
      --keep-hourly <N>          Hourly retention count (default: 24)
      --keep-daily <N>           Daily retention count (default: 7)
      --keep-weekly <N>          Weekly retention count (default: 4)
      --keep-monthly <N>         Monthly retention count (default: 12)
      --keep-yearly <N>          Yearly retention count (default: 1)
      --dry-run                  Simulate actions without writing or deleting files
  -v, --verbose                  Enable detailed execution logging
      --manual                   Display the comprehensive user manual in plain text / ASCII man-page format
      --manual-markdown          Display the comprehensive user manual in Markdown format
  -h, --help                     Print help information
  -V, --version                  Print version information
```

### Configuration File Support
Configuration files eliminate format friction by supporting the **exact same flags and parameter names** used on the command-line:

#### 1. Direct CLI Arguments Format (`.conf` or `.args`)
Write options and paths exactly as they appear on the command line:
```text
# vcopy.conf
--sources /home/user/documents
--sources /home/user/photos
--dest /mnt/usb/backup
--flatten
--preserve-permissions
--keep-daily 7
--keep-weekly 4
```

#### 2. Simple Key-Value Format
Write plain `key = value` lines matching CLI options:
```text
# vcopy.conf
dest = /mnt/usb/backup
sources = /home/user/documents, /home/user/photos
flatten = true
preserve-permissions = true
keep-daily = 7
keep-weekly = 4
```

#### 3. TOML and JSON Format
Supports identical kebab-case and snake_case keys:
```toml
# vcopy.toml
dest = "/mnt/usb/backup"
sources = ["/home/user/documents", "/home/user/photos"]
flatten = true
preserve-permissions = true
verbose = false
dry-run = false
max-versions = 10
keep-hourly = 24
keep-daily = 7
keep-weekly = 4
keep-monthly = 12
keep-yearly = 1
```

Precedence: CLI command-line flags and positional arguments always override settings defined in any configuration file.

---

## 6. Test Suite & Verification

The integration test suite consists of 26 automated tests:

* `tests/integration_tests.rs`:
  * `test_basic_backup_and_versioning`: Verifies initial mirror sync, subsequent modifications, and historical archiving.
  * `test_deletion_and_marker_creation`: Verifies file deletion tracking and `.deleted` marker generation.
  * `test_multiple_sources_mapping`: Verifies prefixed subdirectories for multi-source backups.
  * `test_multiple_sources_with_flatten`: Verifies `--flatten` mode with multiple sources.
  * `test_flatten_collisions_versioning_and_restore`: Verifies stem disambiguation, `.v0`/`.v1` monotonic versioning, and restore.
  * `test_files_from`: Verifies backward compatibility of `--files-from`.
  * `test_paths_from_file_and_stdin`: Verifies source path ingestion via `--paths-from` (files, folders, stdin `-`, and `--sources-from`).
  * `test_multiple_paths_from_and_glob_masks`: Verifies multiple `-f` parameters, comma-separated lists, and glob pattern expansion (`backups*`).
  * `test_config_file_toml_and_json`: Verifies full parameter ingestion via TOML and JSON configuration files.
  * `test_config_cli_arguments_format`: Verifies configuration files written in direct CLI flags format (`--dest`, `--sources`, `--flatten`).
  * `test_config_key_value_and_kebab_case`: Verifies configuration files written in unquoted key-value format with exact kebab-case CLI names.
  * `test_config_cli_overrides`: Verifies CLI arguments override configuration file values.
  * `test_cli_sources_flag`: Verifies `-s / --sources` flag on CLI.
  * `test_manual_embedded_ascii_and_markdown`: Verifies embedded documentation in ASCII, Markdown, and manual subcommands.
  * `test_concurrency_lock`: Verifies concurrent execution blocking via `.lock`.
  * `test_no_changes_does_not_create_snapshot`: Verifies no redundant snapshots created when sources are unchanged.
  * `test_dangerous_overlapping_and_root_paths_rejected`: Verifies safety guards on root and overlapping paths.
  * `test_file_to_directory_and_directory_to_file_replacement`: Verifies replacement handling between file types.
  * `test_manifest_path_traversal_ignored`: Verifies directory traversal defense (`..` paths rejected).
* `tests/prune_tests.rs`:
  * `test_count_based_pruning`: Verifies `-n <N>` snapshot pruning.
  * `test_prune_preserves_older_snapshot_restore`: Verifies reverse-delta consolidation when intermediate snapshots are pruned.
  * `test_flat_pruning_and_consolidation`: Verifies version consolidation and pruning in `--flatten` mode.
  * `test_prune_ignores_non_snapshot_dirs_and_symlinks`: Verifies pruning safety rules on non-snapshot directories.
* `tests/restore_tests.rs`:
  * `test_point_in_time_restore`: Verifies exact historical point-in-time file recovery and timestamp preservation.
  * `test_multi_snapshot_restore_with_later_deletions_and_modifications`: Verifies multi-stage modification and deletion recovery.
  * `test_restore_latest`: Verifies restore of current live mirror.
  * `test_restore_permissions_option`: Verifies mode permission preservation and restoration when `-p` is enabled.
