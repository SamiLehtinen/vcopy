# vcopy

[![CI](https://github.com/samilehtinen/vcopy/actions/workflows/ci.yml/badge.svg)](https://github.com/samilehtinen/vcopy/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/samilehtinen/vcopy?color=blue)](https://github.com/samilehtinen/vcopy/releases)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-green.svg)](LICENSE-MIT)
[![Rust](https://img.shields.io/badge/rust-1.75%2B-orange.svg)](https://www.rust-lang.org)

**Directly browseable, zero-link, multi-platform file sync with snapshot history.**

`vcopy` is a lightweight, zero-dependency CLI backup tool written in pure Rust. It creates directly browseable, uncompressed filesystem mirrors on external drives and non-snapshotting filesystems (exFAT, FAT32, NTFS, ext4, APFS) while preserving a full historical change record.

---

## Key Features & Invariants

* **Directly Browseable Destination**: Live mirror files remain raw, uncompressed, and directly readable in any file manager (Windows Explorer, Finder, Dolphin, etc.) without proprietary backup readers or archive extractors.
* **Zero Links**: Never creates or follows hardlinks, symlinks, or block reflinks. Operates safely across FAT32, exFAT, external USB drives, and cloud-synced folders.
* **Dual Storage Layouts**:
  * **Hierarchical Snapshot Mode (Default)**: Keeps the live mirror at `<dest>/` and archives historical versions under dated directories in `.version-history/<YYYY-MM-DD_HHMMSS>/`.
  * **Single-Directory Flat Mode (`--flatten`)**: Keeps all files (live and historical) directly in a single destination folder using monotonic version suffixes (`.v0`, `.v1`, ...) with automatic collision disambiguation and scan recovery.
* **Nanosecond Timestamps & SIMD CRC32**: Overcomes FAT/exFAT 2-second timestamp drift by persisting exact nanosecond modification timestamps (`mtime_ns`) and fast SIMD CRC32 hashes in `manifest.json` and `manifest.jsonl`.
* **Optional Permission Preservation (`-p / --preserve-permissions`)**: Persists Unix file mode bits and Windows readonly attributes in metadata manifests, allowing faithful restoration across compatible filesystems without encumbering FAT32/exFAT storage.
* **Atomic Staging Writes**: Statically writes all file copies and manifests through temporary files (`.<file>.tmp.<pid>.<id>`) and atomically renames them into place to prevent corruption upon abrupt power loss or cancellation.
* **Safe Pruning (Reverse-Delta Consolidation)**: Pruning intermediate snapshots migrates displaced files needed by surviving older snapshots before deletion, guaranteeing point-in-time restore integrity across GFS retention cycles.
* **Non-Blocking Resilient Advisory Locking**: Uses advisory locking (`.version-history/.lock`) to block overlapping concurrent runs, falling back automatically to an atomic PID-file lock on filesystems that do not support POSIX `flock`.

---

## Storage Modes

### 1. Hierarchical Snapshot Mode (Default)
Ideal for traditional backups where the directory tree is preserved and displaced files are archived into dated snapshot subdirectories:

```text
/mnt/usb/backup/
├── documents/
│   ├── report.pdf                      # Current live version
│   └── notes.txt                       # Current live version
└── .version-history/
    ├── .lock                           # Advisory lock / PID fallback
    ├── baseline.json                   # Current live manifest
    ├── 2026-11-09_083000/              # Historical snapshot folder
    │   ├── manifest.json               # Full manifest for this snapshot
    │   └── documents/
    │       └── report.pdf              # Displaced prior version from 08:30:00
    └── 2026-11-09_090000/              # Historical snapshot folder
        ├── manifest.json
        └── documents/
            ├── old_file.txt            # Displaced file
            └── old_file.txt.deleted    # Zero-byte deletion marker
```

### 2. Single-Directory Flat Mode (`--flatten`)
Ideal when you need all files in a single flat folder (e.g. flat USB drives, photo ingest dropboxes, single-folder cloud buckets):

```text
/mnt/usb/backup/
├── report.pdf                          # Current live version
├── report.v0.pdf                       # Oldest historical version
├── report.v1.pdf                       # Second historical version
├── notes_from_work.txt                 # Disambiguated source collision
├── notes_from_personal.txt             # Disambiguated source collision
├── notes_from_personal.v0.txt          # Historical version of notes_from_personal
└── .version-history/
    ├── .lock                           # Advisory lock
    ├── baseline.json                   # Latest live state manifest
    └── manifest.jsonl                  # Append-only snapshot ledger
```

---

## Installation & Pre-Built Binaries

Pre-compiled standalone static binaries for release `v0.2.0` are available on the [GitHub Releases](https://github.com/samilehtinen/vcopy/releases) page for the following targets:

| Platform | Architecture | Target Triple | Binary Name |
| :--- | :--- | :--- | :--- |
| **Linux (Static MUSL)** | x86_64 | `x86_64-unknown-linux-musl` | `vcopy` |
| **Linux (Static MUSL)** | ARM64 | `aarch64-unknown-linux-musl` | `vcopy` |
| **Windows** | x86_64 | `x86_64-pc-windows-gnullvm` | `vcopy.exe` |
| **Windows** | ARM64 | `aarch64-pc-windows-gnullvm` | `vcopy.exe` |
| **macOS** | Apple Silicon (arm64) | `aarch64-apple-darwin` | `vcopy` |
| **macOS** | Intel (x86_64) | `x86_64-apple-darwin` | `vcopy` |

### Building from Source

Ensure you have Rust 1.75+ installed:

```bash
git clone https://github.com/samilehtinen/vcopy.git
cd vcopy

# Build optimized native release binary
cargo build --release

# Binary is available at target/release/vcopy
./target/release/vcopy --version
```

### Running Tests
```bash
cargo test
```

---

## CLI Usage & Commands

```text
vcopy [OPTIONS] [SOURCES... DEST]... [COMMAND]
```

### 1. Default (Auto Backup & Prune)
Executes a backup snapshot and automatically applies retention policies:

```bash
# Backup single directory
vcopy /home/user/documents /mnt/usb/backup

# Backup multiple directories into prefixed folders
vcopy /home/user/documents /home/user/photos /mnt/usb/backup

# Single-directory flat backup (all files in root with version numbers)
vcopy --flatten /home/user/documents /home/user/photos /mnt/usb/backup

# Read source paths from a text file
vcopy --files-from sources.txt /mnt/usb/backup

# Preserve Unix file modes and Windows readonly attributes
vcopy -p /home/user/code /mnt/usb/backup
```

### 2. Create Snapshot (Without Pruning)
Creates a backup snapshot without running retention pruning:
```bash
vcopy create /path/to/src /mnt/usb/backup
```

### 3. Listing Snapshots
Displays all historical snapshots, transfer statistics, and timestamps:
```bash
vcopy list /mnt/usb/backup
```
Example output:
```text
Snapshot               Copied     Modified   Deleted    Transferred    
----------------------------------------------------------------------
2026-11-09_083015      142        0          0          12491024 bytes 
2026-11-09_091520      3          12         1          452010 bytes   
```

### 4. Point-in-Time Restore
Restores the full state of your files from a specific snapshot or the latest live mirror:

```bash
# Restore latest mirror state
vcopy restore /mnt/usb/backup /path/to/restored

# Restore from a specific point in time (timestamp or prefix)
vcopy restore --target-time 2026-11-09_083015 /mnt/usb/backup /path/to/restored

# Restore with permission restoration
vcopy restore -p --target-time 2026-11-09_083015 /mnt/usb/backup /path/to/restored
```

### 5. Snapshot Pruning & Retention
Manually prune old snapshots using count-based or Grandfather-Father-Son (GFS) policies:

```bash
# Keep only the newest 5 snapshots
vcopy prune -n 5 /mnt/usb/backup

# Custom GFS retention schedule
vcopy prune --keep-hourly 24 --keep-daily 7 --keep-weekly 4 --keep-monthly 12 /mnt/usb/backup

# Preview pruning actions without deleting files (dry run)
vcopy prune -n 5 --dry-run -v /mnt/usb/backup
```

---

## Retention Policy (GFS & Count)

| Flag | Default | Description |
| :--- | :--- | :--- |
| `-n, --max-versions <N>` | None | Retains only the newest $N$ snapshots (overrides GFS). |
| `--keep-hourly <N>` | 24 | Retains one snapshot per hour for the past $N$ hours. |
| `--keep-daily <N>` | 7 | Retains one snapshot per day for the past $N$ days. |
| `--keep-weekly <N>` | 4 | Retains one snapshot per week for the past $N$ weeks. |
| `--keep-monthly <N>` | 12 | Retains one snapshot per month for the past $N$ months. |
| `--keep-yearly <N>` | 1 | Retains one snapshot per year for the past $N$ years. |

---

## Documentation & Developer Resources

* [Engineering Specification (`docs/spec.md`)](docs/spec.md): Complete architecture specification, data schema, collision disambiguation, scan recovery, and verification details.
* [Agent Guidelines (`AGENTS.md`)](AGENTS.md): Environment configuration, cross-compilation instructions, and coding invariants for AI coding agents.

---

## License

Dual-licensed under either:
* Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
* MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.
