# VCOPY(1) User Manual

**vcopy** - Directly browseable, zero-link, multi-platform backup and sync tool with historical versioning.

---

## SYNOPSIS

```text
vcopy [OPTIONS] [SOURCES... DEST] [COMMAND]
vcopy manual [--markdown]
vcopy --manual | --manual-markdown
```

---

## QUICK START & SIMPLE EXAMPLES

### 1. Basic Single Directory Backup
Syncs `/home/user/documents` into `/mnt/usb/backup`:
```bash
vcopy /home/user/documents /mnt/usb/backup
```
The live files appear directly in `/mnt/usb/backup/documents/` as regular uncompressed files. Historical snapshots are archived in `.version-history/`.

---

### 2. Multiple Sources
You can pass multiple source directories or files either positionally or with the `-s / --sources` flag:

**Positional multiple sources:**
```bash
# Destination is always the final argument
vcopy /home/user/docs /home/user/photos /home/user/music /mnt/usb/backup
```
In the destination mirror, distinct top-level source folders are maintained:
```text
/mnt/usb/backup/
├── docs/
├── photos/
└── music/
```

**Using the `-s / --sources` and `-d / --dest` flags:**
```bash
vcopy -s /home/user/docs -s /home/user/photos -d /mnt/usb/backup
```

**Excluding files and directories (`-e / --exclude`, `--exclude-from`, `.vcopyignore`):**
```bash
# Exclude temporary files and build artifacts
vcopy -e "*.tmp" -e "node_modules/" -e "target/" /home/user/code /mnt/usb/backup

# Read exclusion patterns from a file (e.g. .gitignore)
vcopy --exclude-from .gitignore /home/user/code /mnt/usb/backup

# Any .vcopyignore file placed in source directories is automatically applied
```

**Reading sources from files or wildcards (`-f / --paths-from`):**
```bash
# Read paths from a list file (one path per line, # for comments)
vcopy -f project_paths.txt -d /mnt/usb/backup

# Read paths matching file glob patterns
vcopy -f "backups_*.txt" -d /mnt/usb/backup

# Pipe paths directly from find or stdin ('-')
find /home/user/code -name "*.rs" | vcopy -f - -d /mnt/usb/backup
```

---

### 3. Numbered Versions in a Flat Directory (`--flat`)
If you want all files in a single flat directory without subfolder hierarchies (ideal for flat USB thumb drives, camera card ingests, or single-folder cloud storage), use `--flat` (or `--flatten`).

Every file is placed directly in the destination root. Historical revisions receive 1-based monotonic numbered version suffixes (`.v1`, `.v2`, `.v3`, ...):

```bash
vcopy --flat /home/user/docs /home/user/photos /mnt/usb/backup
```

**Destination structure with numbered versions:**
```text
/mnt/usb/backup/
├── report.pdf                     # Current live version (latest)
├── report.v1.pdf                  # First archived historical version (version 1)
├── report.v2.pdf                  # Second archived historical version (version 2)
├── photo.jpg                      # Current live photo
├── photo.v1.jpg                   # Prior version of photo
├── notes.txt                      # First file with basename notes.txt
├── notes_2.txt                    # Stem collision disambiguated (<stem>_2.<ext>)
├── notes_2.v1.txt                 # Historical version of notes_2.txt
├── .version-history.lock          # Advisory concurrency lock
├── .version-history.baseline.json # Live state manifest & checksums
└── .version-history.manifest.jsonl# Snapshot history ledger
```

---

### 4. Zero-Obfuscation Configuration Files (`-c / --config`)
No separate manual or syntax to learn! A configuration file uses the **exact same command-line flags and parameters**:

```bash
vcopy -c backup.conf
```

**Direct CLI arguments format (`backup.conf`):**
```text
# backup.conf - exact same flags as the command line!
--sources /home/user/documents
--sources /home/user/photos
--dest /mnt/usb/backup
--flatten
--preserve-permissions
--keep-daily 7
--keep-weekly 4
```

**Simple key-value format (`backup.conf`):**
```text
dest = /mnt/usb/backup
sources = /home/user/documents, /home/user/photos
flatten = true
preserve-permissions = true
keep-daily = 7
keep-weekly = 4
```

**TOML format (`backup.toml`):**
```toml
dest = "/mnt/usb/backup"
sources = ["/home/user/documents", "/home/user/photos"]
flatten = true
preserve-permissions = true
keep-daily = 7
keep-weekly = 4
```

---

### 5. Listing Snapshots (`list`, `ls`)
View historical snapshots, transfer metrics, and file change counts:
```bash
vcopy ls /mnt/usb/backup
```
Output:
```text
Snapshot                   Copied     Modified   Deleted    Transferred             
------------------------------------------------------------------------------------
2026-11-09_083000          142        0          0          11.9 MB (12,491,024 B)  
2026-11-09_091500          3          12         1          441.4 KB (452,010 B)    
2026-11-09_100000 (live)   0          0          0          0 B                     
```

---

### 6. Point-in-Time & Partial Restore
Restore the entire file tree, a single directory subtree, or an individual file from `latest` or an exact prior snapshot:

```bash
# Restore latest live mirror
vcopy restore /mnt/usb/backup /tmp/restored

# Restore files as they existed at a specific snapshot timestamp
vcopy restore -t 2026-11-09_083000 /mnt/usb/backup /tmp/restored

# Restore a single file only
vcopy restore /mnt/usb/backup /tmp/restored documents/report.pdf

# Restore a directory subtree or wildcard pattern
vcopy restore -i "docs/" /mnt/usb/backup /tmp/restored
vcopy restore --filter "*.pdf" /mnt/usb/backup /tmp/restored

# Restore with POSIX permissions and Windows readonly attributes
vcopy restore -p -t 2026-11-09_083000 /mnt/usb/backup /tmp/restored
```

---

### 7. Integrity Verification & Bit-Rot Detection (`verify`, `check`)
Recalculate checksums to detect disk corruption and bit-rot across live mirrors and historical archives:

```bash
# Full bit-rot check (reads and recomputes SIMD CRC32 for every file)
vcopy verify /mnt/usb/backup

# Quick check (verifies file sizes and timestamps, skipping file content reads)
vcopy check --quick /mnt/usb/backup
```

---

### 8. Retention & Snapshot Pruning
Prune old snapshots using count limits or Grandfather-Father-Son (GFS) schedules:

```bash
# Retain only the newest 5 snapshots
vcopy prune -n 5 /mnt/usb/backup

# Retain 24 hourly, 7 daily, 4 weekly, 12 monthly, and 1 yearly snapshots
vcopy prune --hourly 24 --daily 7 --weekly 4 /mnt/usb/backup

# Dry run: preview which snapshots would be deleted without modifying disk
vcopy prune -n 5 --dry-run -v /mnt/usb/backup
```

---

## PARAMETER & OPTIONS REFERENCE

### Core Options

| Option | Shorthand | Value | Default | Description |
| :--- | :--- | :--- | :--- | :--- |
| `--sources` | `-s` | `<PATH>` | None | Explicit source directory or file. Can be specified multiple times or comma-separated. *(Alias: `--source`)* |
| `--dest` | `-d` | `<PATH>` | None | Destination backup directory. *(Alias: `--destination`)* |
| `--paths-from` | `-f` | `<PATTERN>` | None | Read source paths from text file, file glob (e.g. `backups*`), or stdin (`-`). *(Aliases: `--files-from`, `--sources-from`)* |
| `--config` | `-c` | `<FILE>` | None | Path to configuration file (direct CLI args format, key-value, TOML, or JSON). |
| `--exclude` | `-e` | `<PATTERN>` | None | Exclude files or directories matching glob pattern (`*.tmp`, `node_modules/`, `target/`). Can be specified multiple times. |
| `--exclude-from` | | `<FILE>` | None | Read exclude glob patterns from a file (one per line, e.g. `.gitignore`). |
| `--flat` | | Flag | `false` | Single-directory flat mode: stores all live and historical files in root using 1-based `.v1`, `.v2` version numbers. *(Alias: `--flatten`)* |
| `--flat-date` | | Flag | `false` | In flat mode, use timestamp in archived version names (`<stem>.<TIMESTAMP>.<ext>`) instead of monotonic `.v<N>`. |
| `--perms` | `-p` | Flag | `false` | Record POSIX mode bits (Unix) and readonly attributes (Windows) in manifests and restore them. *(Aliases: `--preserve-permissions`, `--permissions`)* |
| `--time` | `-t` | `<TIMESTAMP>` | `latest` | Snapshot timestamp (`YYYY-MM-DD_HHMMSS` or prefix; 1-second resolution) or `latest` for `restore` or `verify`. *(Aliases: `--target-time`, `--snapshot`)* |
| `--include` | `-i` | `<PATTERN>` | None | In restore mode, restore only files matching path, prefix directory (`docs/`), or glob pattern (`*.pdf`). *(Alias: `--filter`)* |
| `--force` | `-F` | Flag | `false` | Force execution, bypassing safety circuit breakers (100% loss, foreign directory, missing source). |
| `--allow-existing` | | Flag | `false` | Allow destination or restore output directory to contain pre-existing foreign files. |
| `--allow-empty-source` | | Flag | `false` | Allow empty source without tripping the 100% deletion safety circuit breaker. |
| `--allow-missing-source` | | Flag | `false` | Allow missing source paths without aborting the backup run. |
| `--dry-run` | | Flag | `false` | Simulate execution and display planned operations without writing to disk. |
| `--quick` | | Flag | `false` | In verify / check mode, perform quick checks using size/mtime (with 2-second FAT/exFAT tolerance), skipping full file CRC32 recalculation. |
| `--verbose` | `-v` | Flag | `false` | Enable detailed output listing every copied, modified, or deleted file. |
| `--manual` | | Flag | | Print this complete user manual in ASCII / man-page format. *(Alias: `--man`)* |
| `--manual-markdown` | | Flag | | Print this complete user manual in Markdown format. *(Aliases: `--manual-md`, `--man-md`, `--markdown`)* |

### Retention Policy Options (GFS & Count)

| Option | Shorthand / Aliases | Default | Description |
| :--- | :--- | :--- | :--- |
| `--max-versions` | `-n` | None | Retain only the newest $N$ snapshots (overrides GFS schedules). |
| `--keep-hourly` | `--hourly` | 24 | Number of hourly snapshot buckets to retain. |
| `--keep-daily` | `--daily` | 7 | Number of daily snapshot buckets to retain. |
| `--keep-weekly` | `--weekly` | 4 | Number of weekly snapshot buckets to retain. |
| `--keep-monthly` | `--monthly` | 12 | Number of monthly snapshot buckets to retain. |
| `--keep-yearly` | `--yearly` | 1 | Number of yearly snapshot buckets to retain. |

---

## SUBCOMMANDS

- **`auto`** *(default when no subcommand is specified)*:
  Synchronizes source directories into destination, creates a timestamped snapshot, and automatically executes GFS retention pruning.
- **`create`**:
  Creates a new backup snapshot without running retention pruning.
- **`restore [OPTIONS] <DEST> <TARGET_DIR> [PATH_OR_PATTERN]`**:
  Restores files from destination to a specified target directory from `latest` or `-t / --time <TIMESTAMP>`. Supports single-file, subtree, or wildcard filtering via positional argument or `-i / --include / --filter`.
- **`list`** *(alias: `ls`)*:
  Lists all historical snapshots, file counts, and human-readable transferred sizes.
- **`verify`** *(alias: `check`)*:
  Verifies mirror files and snapshots against recorded CRC32 checksums, reporting verified OK, corrupted, missing, and untracked files. Use `--quick` for size/mtime checking.
- **`prune`**:
  Manually prunes historical snapshots according to retention rules with reverse-delta consolidation.
- **`manual`** *(alias: `man`)*:
  Displays this complete user guide. Use `--markdown` for Markdown output.

---

## STORAGE ARCHITECTURE & MODES

### Mode 1: Hierarchical Snapshot Mode (Default)
The live mirror resides at `<dest>/` exactly matching the source hierarchy. When a file is modified or deleted in subsequent runs, the previous version is displaced into a dated snapshot subfolder under `.version-history/<YYYY-MM-DD_HHMMSS>/`:

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

### Mode 2: Single-Directory Flat Mode (`--flat`)
All files (live files, disambiguated sources, and numbered versions) reside in a single flat destination directory:

- **Numbered Versions (1-Based)**: Historical versions receive 1-based monotonic suffixes: `<stem>.v1.<ext>`, `<stem>.v2.<ext>`, ...
- **Timestamped Versions**: Optionally enabled via `--flat-date` to produce `<stem>.<YYYY-MM-DD_HHMMSS>.<ext>` (1-second resolution; sub-second snapshots are not supported).
- **Monotonic Numbering Behavior**: Version numbers are strictly monotonic and are **never shifted or renamed** during snapshot pruning, preventing massive disk I/O thrashing and FAT32/exFAT directory corruption.
- **GFS Retention in Flat Mode**: Retention pruning policies evaluate the snapshot ledger and delete displaced historical versions whose retention buckets have expired without needing to renumber surviving files.
- **Collision Disambiguation**: When multiple files share identical basenames across sources or subdirectories, numeric suffixes are appended (`<stem>_2.<ext>`, `<stem>_3.<ext>`).
- **Metadata**: Stored in root as `.version-history.baseline.json`, `.version-history.manifest.jsonl`, and `.version-history.lock`.
- **Orphan Scan Recovery**: If metadata files are missing, `vcopy` scans the directory on startup, parses all `.v<N>` and timestamped version filenames, and reconstructs the version history automatically.

---

## TECHNICAL INVARIANTS & SAFETY GUARANTEES

1. **Zero Links**:
   Never creates or follows hardlinks, symlinks, or block reflinks. Destination mirrors are 100% portable and fully compatible with FAT32, exFAT, NTFS, ext4, and cloud storage providers.
2. **Directly Browseable Uncompressed Files**:
   Live mirror files and historical versions are stored as standard, uncompressed files readable by any file explorer or utility without special tools.
3. **Atomic Staging Writes**:
   All files and metadata manifests are written to temporary staging files (`.<file>.tmp.<pid>.<id>`) and renamed into place atomically to protect against power loss or mid-process termination.
4. **Nanosecond Timestamps & SIMD CRC32**:
   FAT32 and exFAT store timestamps with 2-second resolution drift. `vcopy` calculates exact nanosecond modification times and fast SIMD-accelerated CRC32 hashes stored in `manifest.json` to prevent unnecessary recopying.
5. **Reverse-Delta Pruning Consolidation**:
   When intermediate snapshots are pruned, displaced files needed by surviving older snapshots are automatically consolidated and migrated into the preceding kept snapshot before deletion. No restore point is ever broken.
6. **Advisory Locking with PID Fallback**:
   Concurrently running backups on the same destination are blocked using `.lock`. If the filesystem lacks POSIX `flock` support (e.g. FUSE mounts), an atomic PID lockfile is used automatically.
7. **100% Wipeout Circuit Breaker**:
   If source directories are missing, empty, or unmounted, `vcopy` halts immediately to prevent deleting 100% of files in the destination mirror. Override with `--allow-empty-source` or `-F / --force`.
8. **Protected System & Foreign Directory Guards**:
   `vcopy` refuses to modify root (`/`), user home, system directories, or foreign populated folders that do not contain `vcopy` history. Override with `--allow-existing` or `-F / --force`.

---

## AUTHORS & LICENSE

Created by **Sami Lehtinen** (`noreply@sami-lehtinen.net`).
Released under the **Zero-Clause BSD (0BSD)** License.
GitHub Repository: <https://github.com/samilehtinen/vcopy>
