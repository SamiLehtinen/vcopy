# `vcopy` Tasks & Roadmap

## 1. Exclude & Ignore System (`--exclude`)
- [ ] Add `-e, --exclude <PATTERN>` CLI flag (multi-value, supports glob patterns like `*.tmp`, `node_modules/`, `target/`, `.DS_Store`).
- [ ] Add `--exclude-from <FILE>` to read ignore patterns from a file (e.g., `.gitignore` or custom ignore lists).
- [ ] Support `.vcopyignore` files placed directly in source directories.
- [ ] Wire exclusion filters into directory walkers for both hierarchical sync (`sync.rs`) and flat mode (`flatten.rs`).
- [ ] Record exclusion rules in `manifest.json` / `baseline.json` so prune and restore understand omitted patterns.
- [ ] Add unit and integration tests for pattern matching, nested folder excludes, and wildcard handling.

## 2. Integrity Verification & Bit-Rot Detection (`vcopy verify` / `check`)
- [ ] Add `verify` (alias: `check`) subcommand: `vcopy verify [OPTIONS] <DEST>`.
- [ ] Verify live mirror files against CRC32 checksums recorded in `baseline.json`.
- [ ] Verify historical snapshot files against CRC32 checksums recorded in `manifest.json` (hierarchical) and `.manifest.jsonl` (flat).
- [ ] Provide clear visual reporting:
  - OK / Verified count.
  - Corrupted / Mismatched files (bit-rot detection with expected vs. actual CRC32).
  - Missing files (recorded in manifest but absent on disk).
  - Orphan / Untracked files in destination.
- [ ] Support `--quick` (size + mtime only) and `--full` (read and recompute CRC32, default).
- [ ] Return non-zero exit code if integrity verification detects any corruptions.

## 3. Partial & Single-Path Restore
- [ ] Enhance `vcopy restore` to accept optional path filters or subpaths:
  - Syntax: `vcopy restore [OPTIONS] <DEST> <TARGET_DIR> [PATH_OR_PATTERN]`
  - Flag option: `--include <PATTERN>` or `--filter <GLOB>`.
- [ ] Allow restoring an individual file (e.g., `vcopy restore /mnt/backup /tmp/restored documents/report.pdf`).
- [ ] Allow restoring a single directory subtree rather than forcing a 100% full mirror restoration.
- [ ] Support partial restore in both hierarchical mode and single-directory `--flat` mode.
- [ ] Add integration tests verifying single-file restore, subtree restore, and wildcard restore.

## 4. Flat Mode Version Naming & GFS Documentation
- [ ] Document how GFS retention policies interact with flat numbered versions (`.v0`, `.v1`, ...).
- [ ] Clarify monotonic numbering behavior: why versions are not renamed on prune to avoid massive disk I/O thrashing on FAT32/exFAT.
- [ ] Add support / option for timestamped versions in flat mode (e.g., `--flat-date` or `--flat-naming=date` producing `<stem>.<YYYY-MM-DD_HHMMSS>.<ext>`) alongside monotonic `.v<N>`.
- [ ] Ensure orphan scan recovery seamlessly handles both monotonic `.v<N>` and timestamp-based `.YYYY-MM-DD_HHMMSS` flat filenames.

## 5. Human-Readable Sizes & Metrics Output
- [ ] Add human-readable byte formatting helper in `util.rs` (e.g. `format_size(bytes)` -> `11.9 MB (12,491,024 bytes)`).
- [ ] Update `vcopy list` / `vcopy ls` table output to display human-readable sizes in the `Transferred` column.
- [ ] Update backup summary log (`RunSummary`) to display human-readable transfer totals.
- [ ] Update dry-run output to show human-readable file sizes.

## 6. Update All Documentation, Man Pages & Embedded Manuals
- [ ] Update examples across all docs (`README.md`, `docs/MANUAL.md`, `docs/manual.txt`, `docs/spec.md`, `docs/vcopy.1`) to use modern shortened flags:
  - `--flat` (instead of legacy `--flatten`).
  - `-p, --perms` (instead of `--preserve-permissions`).
  - `-t, --time` (instead of `--target-time`).
  - `--daily`, `--hourly`, `--weekly`, `--monthly`, `--yearly` (instead of `--keep-*`).
  - `vcopy ls` (highlighted alongside `vcopy list`).
- [ ] Fix duplicate `manual` subcommand entry in `docs/manual.txt`.
- [ ] Document newly added safety circuit breaker flags:
  - `-F, --force`
  - `--allow-existing`
  - `--allow-empty-source`
  - `--allow-missing-source`
- [ ] Add the 100% Wipeout Circuit Breaker and Protected Directory Guards to the "Technical Invariants & Safety Guarantees" sections in all documentation.

---

## Previously Completed
- [x] Reorganize readme file: usage first, how it works, tech details at end.
- [x] Embed comprehensive manual (ASCII and Markdown) into the binary with `--manual`, `--manual-markdown`, and `manual` commands.
- [x] Include simple examples with numbered versions and multiple sources near top.
- [x] Orphan scan recovery for flat mode (`.v<N>`) and hierarchical mode without manifests.
- [x] Safety circuit breakers (100% loss prevention, unmounted/missing source protection, system/home directory guards).
- [x] Shortened CLI flags (`--flat`, `-p`, `-t`, `-F`, `vcopy ls`, `--daily`, etc.).
- [x] Self-describing `vcopy` comment in manifest metadata headers.
