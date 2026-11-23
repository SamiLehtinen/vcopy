# Changelog

All notable changes to `vcopy` starting from the initial public binary release (`v0.3.0`) are documented in this file.

## [Unreleased]

### Added
- **Decoupled 3-slot ring-buffer copy pipeline (`>= 2 MiB`):** Added `src/copy_pipeline.rs` implementing a persistent single-writer `3 × 1 MiB` reusable heap ring-buffer pipeline for Tier 3 files (`>= 2 MiB`), allowing concurrent source reads and destination writes while keeping `< 1 MiB` (Tier 1, `64 KiB` stack) and `1–2 MiB` (Tier 2, `1 MiB` sync) overhead-free and preserving all `copy_atomic` invariants.

### Fixed
- **Windows advisory lock contention:** Replaced `fs2::FileExt::try_lock_exclusive` on Windows with a `share_mode(0)` + `FILE_FLAG_DELETE_ON_CLOSE` lock handle so concurrent processes reliably detect active locks and clean up `.lock` automatically on exit or crash, and matched `fs2::lock_contended_error()` on Unix.

### Changed
- **CI/CD workflows:** Upgraded `actions/checkout` to `v7` and `softprops/action-gh-release` to `v3` (Node 24 runtime), and configured `release.yml` to publish release notes from `CHANGELOG.md`.

## [0.3.4] - 2026-11-21 — Streaming copy with preallocation, rolling cache eviction & lock cleanup

### Changed
- **Streaming copy with preallocation and rolling page-cache eviction:** Replaced `std::io::copy` in `copy_atomic` with explicit userspace streaming (`64 KiB` buffer for files `< 1 MiB`; `1 MiB` buffer for `>= 1 MiB`) to guarantee no same-filesystem `copy_file_range` reflinks and prevent large backups from evicting active RAM working sets or stalling on dirty-page flushes to external/USB/SMR drives.
  - **Linux:** Uses `posix_fadvise` (`SEQUENTIAL`, `NOREUSE`, and a rolling `1 MiB` `DONTNEED` window) plus best-effort `fallocate(2)` preallocation (avoiding glibc `posix_fallocate(3)` zero-fill overhead on FAT32/exFAT/FUSE).
  - **Windows:** Opens large files with `FILE_FLAG_SEQUENTIAL_SCAN` and best-effort `set_len` preallocation, trimming trailing preallocated bytes if a source shrinks concurrently.
- **Transient lock file cleanup:** `.version-history/.lock` (hierarchical) and `.version-history.lock` (flat) are now automatically removed upon completion or error exit with race-safe handoff, keeping idle backup destinations clean.

### Fixed
- **Concurrent first-run safety check:** Eliminated a TOCTOU race in `is_foreign_populated_directory` when two processes initialize a destination concurrently.

## [0.3.3] - 2026-11-17 — Add -l / --keep-last retention supplement

### Added
- **Keep-last retention supplement (`-l`, `--keep-last <N>`, alias `--last`):** Always keeps the newest `N` snapshots **in addition** to active GFS buckets (`hourly`/`daily`/`weekly`/`monthly`/`yearly`) in both hierarchical and `--flat` modes, preventing burst or irregular backup runs from being pruned too early without discarding long-term GFS history. Supported on the CLI and across all config formats.

## [0.3.2] - 2026-11-16 — Typo-tolerant CLI and config suggestions

### Added
- **Typo-tolerant CLI and config suggestions (`"Did you mean ...?"`):** Added pure-Rust Damerau-Levenshtein matching to suggest corrections for mistyped subcommands (without intercepting real filesystem paths), misplaced or mistyped flags after positional arguments, subcommands passed as flags (`--prune`), and unknown keys or `command` values in CLI, key-value, TOML, and JSON config files.

## [0.3.1] - 2026-11-16 — Add -0 / --no-defaults retention flag

### Added
- **Zero-defaults retention flag (`-0`, `--no-defaults`, aliases `--zero-defaults`, `--no-keep-defaults`):** Defaults any unspecified `--keep-*` GFS retention bucket to `0` instead of `24h / 7d / 4w / 12m / 1y`, making minimal rules like `vcopy prune -0 --daily 7 /dest` concise and unambiguous.

## [0.3.0] - 2026-11-13 — Initial public multi-platform binary release

### Added
- **Initial public multi-platform binary release:** Pre-built static/native release binaries for Linux (`x86_64`, `aarch64` static musl), Windows (`x86_64`, `aarch64`), and macOS (`aarch64`, `x86_64`) with `.sha256` checksums.
- **Browseable, link-free versioned mirrors:**
  - Hierarchical snapshot mode (`.version-history/<YYYY-MM-DD_HHMMSS>/`) and single-directory `--flat` / `--flat-date` modes with 1-based monotonic `.v<N>` versioning and collision disambiguation.
  - Reverse-delta GFS and `--max-versions` (`-n`) pruning consolidation.
  - Point-in-time and partial `restore` (single file, subtree, `-i / --include`), SIMD CRC32 `verify` / `check` (`--quick` with 2-second FAT/exFAT tolerance), `.vcopyignore` / `-e` exclusions, heuristic orphan scan recovery, safety circuit breakers, zero-obfuscation configs, and embedded ASCII/Markdown manuals (`--manual`, `--manual-markdown`).
