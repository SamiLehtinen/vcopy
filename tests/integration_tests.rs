mod common;
use common::TestContext;
use std::fs::{self, File};
use std::io::Write;
use std::process::Command;
use std::thread;
use std::time::Duration;

#[test]
fn test_basic_backup_and_versioning() {
    let ctx = TestContext::new("basic");
    ctx.write_src_file("test.txt", b"version 1\n");

    // Run auto backup
    let status = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(status.success());
    assert!(ctx.dst.join("test.txt").exists());
    assert_eq!(ctx.read_dst_file("test.txt").unwrap(), b"version 1\n");

    // Modify file
    thread::sleep(Duration::from_millis(1050));
    ctx.write_src_file("test.txt", b"version 2\n");

    let status = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(status.success());

    // Verify history directory exists and contains version 1
    let hist = ctx.dst.join(".version-history");
    assert!(hist.is_dir());
    let snapshots: Vec<_> = fs::read_dir(&hist)
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_dir())
        .collect();
    assert_eq!(snapshots.len(), 1);

    let hist_file = snapshots[0].path().join("test.txt");
    assert!(hist_file.exists());
    let content = fs::read_to_string(hist_file).unwrap();
    assert_eq!(content, "version 1\n");

    // Verify live mirror contains version 2
    assert_eq!(ctx.read_dst_file("test.txt").unwrap(), b"version 2\n");
}

#[test]
fn test_deletion_and_marker_creation() {
    let ctx = TestContext::new("deletion");
    ctx.write_src_file("keep.txt", b"staying");
    ctx.write_src_file("delete_me.txt", b"ephemeral");

    // Baseline run
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());
    assert!(ctx.dst.join("delete_me.txt").exists());

    // Wait past timestamp threshold & delete source file
    thread::sleep(Duration::from_millis(1050));
    fs::remove_file(ctx.src.join("delete_me.txt")).unwrap();

    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());

    // Live destination file must be gone
    assert!(!ctx.dst.join("delete_me.txt").exists());
    assert!(ctx.dst.join("keep.txt").exists());

    // History must contain displaced file and zero-byte .deleted marker
    let hist_dir = ctx.dst.join(".version-history");
    let mut snapshots: Vec<_> = fs::read_dir(&hist_dir)
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_dir())
        .collect();
    assert_eq!(snapshots.len(), 1);

    let snap_dir = snapshots.pop().unwrap().path();
    let hist_file = snap_dir.join("delete_me.txt");
    let marker = snap_dir.join("delete_me.txt.deleted");

    assert!(hist_file.is_file());
    assert_eq!(fs::read(hist_file).unwrap(), b"ephemeral");
    assert!(marker.is_file());
    assert_eq!(fs::metadata(marker).unwrap().len(), 0);
}

#[test]
fn test_multiple_sources_mapping() {
    let base = std::env::temp_dir().join(format!(
        "vcopy_test_multisrc_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    ));
    let _ = fs::remove_dir_all(&base);

    let src1 = base.join("src_a");
    let src2 = base.join("src_b");
    let dst = base.join("dest");

    fs::create_dir_all(&src1).unwrap();
    fs::create_dir_all(&src2).unwrap();
    fs::create_dir_all(&dst).unwrap();

    fs::write(src1.join("data_a.txt"), b"source alpha").unwrap();
    fs::write(src2.join("data_b.txt"), b"source beta").unwrap();

    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&src1)
        .arg(&src2)
        .arg(&dst)
        .status()
        .unwrap();
    assert!(st.success());

    // Multi-source without --flatten maps into subdirectories named after sources
    assert!(dst.join("src_a").join("data_a.txt").is_file());
    assert!(dst.join("src_b").join("data_b.txt").is_file());

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn test_multiple_sources_with_flatten() {
    let base = std::env::temp_dir().join(format!(
        "vcopy_test_flatten_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    ));
    let _ = fs::remove_dir_all(&base);

    let src1 = base.join("dir1");
    let src2 = base.join("dir2");
    let dst = base.join("dest");

    fs::create_dir_all(&src1).unwrap();
    fs::create_dir_all(&src2).unwrap();
    fs::create_dir_all(&dst).unwrap();

    fs::write(src1.join("doc1.txt"), b"doc 1").unwrap();
    fs::write(src2.join("doc2.txt"), b"doc 2").unwrap();

    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("--flatten")
        .arg(&src1)
        .arg(&src2)
        .arg(&dst)
        .status()
        .unwrap();
    assert!(st.success());

    assert!(dst.join("doc1.txt").is_file());
    assert!(dst.join("doc2.txt").is_file());

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn test_files_from() {
    let ctx = TestContext::new("files_from");
    ctx.write_src_file("a.txt", b"file a");
    ctx.write_src_file("b.txt", b"file b");

    let list_file = ctx.dir.join("sources.txt");
    let mut f = File::create(&list_file).unwrap();
    writeln!(f, "# Comments should be ignored").unwrap();
    writeln!(f, "{}", ctx.src.join("a.txt").display()).unwrap();
    writeln!(f, "{}", ctx.src.join("b.txt").display()).unwrap();

    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("--files-from")
        .arg(&list_file)
        .arg("--flatten")
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());

    assert_eq!(ctx.read_dst_file("a.txt").unwrap(), b"file a");
    assert_eq!(ctx.read_dst_file("b.txt").unwrap(), b"file b");
}

#[test]
fn test_concurrency_lock() {
    use fs2::FileExt;

    let ctx = TestContext::new("locking");
    ctx.write_src_file("payload.bin", &[0u8; 1024]);

    // Acquire advisory lock beforehand
    let hist = ctx.dst.join(".version-history");
    fs::create_dir_all(&hist).unwrap();
    let lock_file = File::create(hist.join(".lock")).unwrap();
    lock_file.lock_exclusive().unwrap();

    // Spawning vcopy while lock is held should fail or block
    let output = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .output()
        .unwrap();

    assert!(!output.status.success() || !ctx.dst.join("payload.bin").exists());

    lock_file.unlock().unwrap();
}

#[test]
fn test_no_changes_does_not_create_snapshot() {
    let ctx = TestContext::new("no_changes");
    ctx.write_src_file("steady.txt", b"constant");

    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());

    // Destination created, but no snapshot history needed for unchanged baseline initial copy
    let hist = ctx.dst.join(".version-history");
    let count1 = if hist.is_dir() {
        fs::read_dir(&hist)
            .unwrap()
            .flatten()
            .filter(|e| e.path().is_dir())
            .count()
    } else {
        0
    };

    // Second run with no source changes
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());

    let count2 = if hist.is_dir() {
        fs::read_dir(&hist)
            .unwrap()
            .flatten()
            .filter(|e| e.path().is_dir())
            .count()
    } else {
        0
    };

    assert_eq!(count1, count2);
}

#[test]
fn test_dangerous_overlapping_and_root_paths_rejected() {
    let ctx = TestContext::new("path_safety");
    ctx.write_src_file("important.txt", b"do not lose");

    // 1. src == dst must fail
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.src)
        .output()
        .unwrap();
    assert!(!st.status.success());
    assert!(ctx.src.join("important.txt").is_file());

    // 2. dst inside src must fail
    let dst_in_src = ctx.src.join("nested_backup");
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&dst_in_src)
        .output()
        .unwrap();
    assert!(!st.status.success());
    assert!(!dst_in_src.exists());

    // 3. src inside dst must fail and MUST NOT delete or move src files
    let sub_src = ctx.dst.join("sub_source");
    fs::create_dir_all(&sub_src).unwrap();
    fs::write(sub_src.join("precious.txt"), b"keep safe").unwrap();
    fs::write(ctx.dst.join("other.txt"), b"other file").unwrap();

    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&sub_src)
        .arg(&ctx.dst)
        .output()
        .unwrap();
    assert!(!st.status.success());
    assert_eq!(
        fs::read_to_string(sub_src.join("precious.txt")).unwrap(),
        "keep safe"
    );
    assert_eq!(
        fs::read_to_string(ctx.dst.join("other.txt")).unwrap(),
        "other file"
    );

    // 4. Filesystem root "/" as destination must fail
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg("/")
        .output()
        .unwrap();
    assert!(!st.status.success());

    // 5. Restore with target_out == dst or inside dst must fail
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("restore")
        .arg(&ctx.dst)
        .arg(&ctx.dst)
        .output()
        .unwrap();
    assert!(!st.status.success());

    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("restore")
        .arg(&ctx.dst)
        .arg(ctx.dst.join("restored_inside"))
        .output()
        .unwrap();
    assert!(!st.status.success());
}

#[test]
fn test_file_to_directory_and_directory_to_file_replacement() {
    let ctx = TestContext::new("file_dir_swap");

    // Run 1: "morph" is a regular file
    ctx.write_src_file("morph", b"i am a file");
    assert!(Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap()
        .success());
    assert!(ctx.dst.join("morph").is_file());

    // Run 2: "morph" becomes a directory containing "inner.txt"
    fs::remove_file(ctx.src.join("morph")).unwrap();
    ctx.write_src_file("morph/inner.txt", b"i am inside dir");
    assert!(Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap()
        .success());
    assert!(ctx.dst.join("morph").is_dir());
    assert_eq!(
        ctx.read_dst_file("morph/inner.txt").unwrap(),
        b"i am inside dir"
    );

    // Run 3: "morph" becomes a regular file again
    fs::remove_dir_all(ctx.src.join("morph")).unwrap();
    ctx.write_src_file("morph", b"file again");
    assert!(Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap()
        .success());
    assert!(ctx.dst.join("morph").is_file());
    assert_eq!(ctx.read_dst_file("morph").unwrap(), b"file again");
}

#[test]
fn test_manifest_path_traversal_ignored() {
    let ctx = TestContext::new("traversal_guard");
    let victim = ctx.dir.join("outside_victim.txt");
    fs::write(&victim, b"untouchable").unwrap();

    ctx.write_src_file("legit.txt", b"v1");
    assert!(Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap()
        .success());

    // Poison baseline.json with a "../outside_victim.txt" path entry
    let baseline_path = ctx.dst.join(".version-history").join("baseline.json");
    let content = fs::read_to_string(&baseline_path).unwrap();
    let poisoned = content.replace(
        "\"state\": [",
        "\"state\": [\n    {\n      \"path\": \"../outside_victim.txt\",\n      \"size\": 11,\n      \"mtime_ns\": 1000\n    },",
    );
    fs::write(&baseline_path, poisoned).unwrap();

    // Run sync again: must NOT delete or move ../outside_victim.txt
    ctx.write_src_file("legit.txt", b"v2");
    assert!(Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap()
        .success());

    assert!(victim.is_file());
    assert_eq!(fs::read_to_string(&victim).unwrap(), "untouchable");
}

#[test]
fn test_flatten_collisions_versioning_and_restore() {
    let base = std::env::temp_dir().join(format!(
        "vcopy_test_flat_full_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    ));
    let _ = fs::remove_dir_all(&base);

    let src1 = base.join("dir1");
    let src2 = base.join("dir2");
    let dst = base.join("dst");
    let restore_out = base.join("restored");

    fs::create_dir_all(src1.join("sub")).unwrap();
    fs::create_dir_all(&src2).unwrap();
    fs::create_dir_all(&dst).unwrap();

    // Create overlapping filenames across directories
    fs::write(src1.join("sub").join("notes.txt"), b"dir1 sub notes v1").unwrap();
    fs::write(src2.join("notes.txt"), b"dir2 notes v1").unwrap();
    fs::write(src1.join("unique.txt"), b"unique file v1").unwrap();

    // 1. Initial backup with --flatten
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("--flatten")
        .arg(&src1)
        .arg(&src2)
        .arg(&dst)
        .status()
        .unwrap();
    assert!(st.success());

    // Both files must exist in flat dst without collision!
    assert!(dst.join("notes.txt").is_file());
    assert!(dst.join("notes_2.txt").is_file());
    assert!(dst.join("unique.txt").is_file());
    assert!(dst.join(".version-history.baseline.json").is_file());
    assert!(dst.join(".version-history.manifest.jsonl").is_file());
    assert!(!dst.join(".version-history").exists());

    std::thread::sleep(std::time::Duration::from_millis(1100));

    // 2. Modify notes in dir1 and delete unique.txt in dir1
    fs::write(src1.join("sub").join("notes.txt"), b"dir1 sub notes v2").unwrap();
    fs::remove_file(src1.join("unique.txt")).unwrap();

    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&src1)
        .arg(&src2)
        .arg(&dst)
        .status()
        .unwrap();
    assert!(st.success());

    // Modified version should be archived as .v1 file in the flat root
    let notes_v1_exists =
        dst.join("notes.v1.txt").is_file() || dst.join("notes_2.v1.txt").is_file();
    assert!(notes_v1_exists);
    assert!(dst.join("unique.v1.txt").is_file());
    assert!(!dst.join("unique.txt").exists());

    // 3. Restore latest: should reconstruct original directory tree
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("restore")
        .arg(&dst)
        .arg(&restore_out)
        .status()
        .unwrap();
    assert!(st.success());

    assert_eq!(
        fs::read(restore_out.join("dir1").join("sub").join("notes.txt")).unwrap(),
        b"dir1 sub notes v2"
    );
    assert_eq!(
        fs::read(restore_out.join("dir2").join("notes.txt")).unwrap(),
        b"dir2 notes v1"
    );
    assert!(!restore_out.join("dir1").join("unique.txt").exists());

    // 4. Restore point-in-time from initial snapshot
    let flat_content = fs::read_to_string(dst.join(".version-history.baseline.json")).unwrap();
    let flat_json: serde_json::Value = serde_json::from_str(&flat_content).unwrap();
    let first_snap_name = flat_json["snapshots"][0]["snapshot_name"].as_str().unwrap();

    let restore_v1_out = base.join("restored_v1");
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("restore")
        .arg("--target-time")
        .arg(first_snap_name)
        .arg(&dst)
        .arg(&restore_v1_out)
        .status()
        .unwrap();
    assert!(st.success());

    assert_eq!(
        fs::read(restore_v1_out.join("dir1").join("sub").join("notes.txt")).unwrap(),
        b"dir1 sub notes v1"
    );
    assert_eq!(
        fs::read(restore_v1_out.join("dir2").join("notes.txt")).unwrap(),
        b"dir2 notes v1"
    );
    assert_eq!(
        fs::read(restore_v1_out.join("dir1").join("unique.txt")).unwrap(),
        b"unique file v1"
    );

    // 5. Test scan-recovery when .v1 file is manually renamed in flat destination
    // Even with live notes.txt (v2) present on disk, restoring snapshot 1 must NOT
    // grab live notes.txt (v2), and must instead scan-recover the renamed v1 file!
    let renamed_v1 = dst.join("manually_renamed_old_notes.bak");
    if dst.join("notes.v1.txt").is_file() {
        fs::rename(dst.join("notes.v1.txt"), &renamed_v1).unwrap();
    } else {
        fs::rename(dst.join("notes_2.v1.txt"), &renamed_v1).unwrap();
    }

    let restore_scan_out = base.join("restored_scan_recovery");
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("restore")
        .arg("--target-time")
        .arg(first_snap_name)
        .arg(&dst)
        .arg(&restore_scan_out)
        .status()
        .unwrap();
    assert!(st.success());
    assert_eq!(
        fs::read(restore_scan_out.join("dir1").join("sub").join("notes.txt")).unwrap(),
        b"dir1 sub notes v1"
    );

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn test_paths_from_file_and_stdin() {
    let ctx = TestContext::new("paths_from_test");
    ctx.write_src_file("doc1.txt", b"doc 1 content");
    ctx.write_src_file("sub/doc2.txt", b"doc 2 content");

    let sub_dir = ctx.src.join("sub");
    let doc1_file = ctx.src.join("doc1.txt");

    // 1. Test --paths-from with a file containing both a file and a directory path
    let list_file = ctx.dir.join("paths_list.txt");
    let mut f = File::create(&list_file).unwrap();
    writeln!(f, "# Header comment").unwrap();
    writeln!(f, "{}", doc1_file.display()).unwrap();
    writeln!(f, "{}", sub_dir.display()).unwrap();

    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("--paths-from")
        .arg(&list_file)
        .arg("--flatten")
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());
    assert_eq!(ctx.read_dst_file("doc1.txt").unwrap(), b"doc 1 content");
    assert_eq!(ctx.read_dst_file("doc2.txt").unwrap(), b"doc 2 content");

    // 2. Test --paths-from - with stdin piping
    let dst2 = ctx.dir.join("dst2");
    let mut child = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("--paths-from")
        .arg("-")
        .arg("--flatten")
        .arg(&dst2)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    {
        let mut stdin = child.stdin.take().unwrap();
        writeln!(stdin, "# Stdin comment").unwrap();
        writeln!(stdin, "{}", doc1_file.display()).unwrap();
        writeln!(stdin, "{}", sub_dir.display()).unwrap();
    }

    let status = child.wait().unwrap();
    assert!(status.success());
    assert_eq!(fs::read(dst2.join("doc1.txt")).unwrap(), b"doc 1 content");
    assert_eq!(fs::read(dst2.join("doc2.txt")).unwrap(), b"doc 2 content");

    // 3. Test --sources-from alias
    let dst3 = ctx.dir.join("dst3");
    let st3 = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("--sources-from")
        .arg(&list_file)
        .arg("--flatten")
        .arg(&dst3)
        .status()
        .unwrap();
    assert!(st3.success());
    assert_eq!(fs::read(dst3.join("doc1.txt")).unwrap(), b"doc 1 content");
}

#[test]
fn test_config_file_toml_and_json() {
    let ctx = TestContext::new("config_test");
    ctx.write_src_file("cfg_item.txt", b"from config");

    let dst_toml = ctx.dir.join("dst_toml");
    let toml_config = ctx.dir.join("vcopy.toml");
    let mut f = File::create(&toml_config).unwrap();
    writeln!(f, "sources = [\"{}\"]", ctx.src.display()).unwrap();
    writeln!(f, "dest = \"{}\"", dst_toml.display()).unwrap();
    writeln!(f, "flatten = false").unwrap();
    writeln!(f, "verbose = true").unwrap();
    writeln!(f, "keep_hourly = 10").unwrap();

    // Run vcopy -c vcopy.toml without any positional arguments
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("-c")
        .arg(&toml_config)
        .status()
        .unwrap();
    assert!(st.success());
    assert_eq!(
        fs::read(dst_toml.join("cfg_item.txt")).unwrap(),
        b"from config"
    );

    // Test JSON configuration file
    let dst_json = ctx.dir.join("dst_json");
    let json_config = ctx.dir.join("vcopy.json");
    let mut f2 = File::create(&json_config).unwrap();
    writeln!(
        f2,
        r#"{{
            "sources": ["{}"],
            "dest": "{}",
            "flatten": true,
            "verbose": true
        }}"#,
        ctx.src.display(),
        dst_json.display()
    )
    .unwrap();

    let st2 = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("--config")
        .arg(&json_config)
        .status()
        .unwrap();
    assert!(st2.success());
    assert_eq!(
        fs::read(dst_json.join("cfg_item.txt")).unwrap(),
        b"from config"
    );
}

#[test]
fn test_config_cli_overrides() {
    let ctx = TestContext::new("config_overrides");
    ctx.write_src_file("overridden.txt", b"override content");

    let dst_cfg = ctx.dir.join("dst_cfg");
    let dst_override = ctx.dir.join("dst_override");
    let toml_config = ctx.dir.join("vcopy.toml");
    let mut f = File::create(&toml_config).unwrap();
    writeln!(f, "sources = [\"{}\"]", ctx.src.display()).unwrap();
    writeln!(f, "dest = \"{}\"", dst_cfg.display()).unwrap();
    writeln!(f, "dry_run = false").unwrap();

    // 1. CLI --dry-run overrides config dry_run = false
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("-c")
        .arg(&toml_config)
        .arg("--dry-run")
        .status()
        .unwrap();
    assert!(st.success());
    assert!(!dst_cfg.exists());

    // 2. CLI --dest overrides config dest
    let st2 = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("-c")
        .arg(&toml_config)
        .arg("--dest")
        .arg(&dst_override)
        .status()
        .unwrap();
    assert!(st2.success());
    assert!(dst_override.join("overridden.txt").exists());
}

#[test]
fn test_multiple_paths_from_and_glob_masks() {
    let ctx = TestContext::new("multi_paths_glob");
    ctx.write_src_file("item1.txt", b"item 1");
    ctx.write_src_file("item2.txt", b"item 2");
    ctx.write_src_file("item3.txt", b"item 3");

    let list_a = ctx.dir.join("backups_alpha.txt");
    let mut f_a = File::create(&list_a).unwrap();
    writeln!(f_a, "{}", ctx.src.join("item1.txt").display()).unwrap();

    let list_b = ctx.dir.join("backups_beta.txt");
    let mut f_b = File::create(&list_b).unwrap();
    writeln!(f_b, "{}", ctx.src.join("item2.txt").display()).unwrap();

    let list_extra = ctx.dir.join("extra_sources.txt");
    let mut f_e = File::create(&list_extra).unwrap();
    writeln!(f_e, "{}", ctx.src.join("item3.txt").display()).unwrap();

    // 1. Multiple -f arguments: -f list_a -f list_extra
    let dst1 = ctx.dir.join("dst1");
    let st1 = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("-f")
        .arg(&list_a)
        .arg("-f")
        .arg(&list_extra)
        .arg("--flatten")
        .arg(&dst1)
        .status()
        .unwrap();
    assert!(st1.success());
    assert_eq!(fs::read(dst1.join("item1.txt")).unwrap(), b"item 1");
    assert_eq!(fs::read(dst1.join("item3.txt")).unwrap(), b"item 3");
    assert!(!dst1.join("item2.txt").exists());

    // 2. Glob wildcard mask: -f "backups_*.txt"
    let dst2 = ctx.dir.join("dst2");
    let mask_pattern = format!("{}/backups_*.txt", ctx.dir.display());
    let st2 = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("-f")
        .arg(&mask_pattern)
        .arg("--flatten")
        .arg(&dst2)
        .status()
        .unwrap();
    assert!(st2.success());
    assert_eq!(fs::read(dst2.join("item1.txt")).unwrap(), b"item 1");
    assert_eq!(fs::read(dst2.join("item2.txt")).unwrap(), b"item 2");
    assert!(!dst2.join("item3.txt").exists());

    // 3. Config file with array of glob masks
    let dst3 = ctx.dir.join("dst3");
    let cfg_file = ctx.dir.join("multi_glob.toml");
    let mut f_cfg = File::create(&cfg_file).unwrap();
    writeln!(
        f_cfg,
        "paths_from = [\"{}\", \"{}\"]",
        mask_pattern,
        list_extra.display()
    )
    .unwrap();
    writeln!(f_cfg, "dest = \"{}\"", dst3.display()).unwrap();
    writeln!(f_cfg, "flatten = true").unwrap();

    let st3 = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("-c")
        .arg(&cfg_file)
        .status()
        .unwrap();
    assert!(st3.success());
    assert_eq!(fs::read(dst3.join("item1.txt")).unwrap(), b"item 1");
    assert_eq!(fs::read(dst3.join("item2.txt")).unwrap(), b"item 2");
    assert_eq!(fs::read(dst3.join("item3.txt")).unwrap(), b"item 3");
}

#[test]
fn test_config_cli_arguments_format() {
    let ctx = TestContext::new("config_cli_args");
    ctx.write_src_file("cli_arg_file.txt", b"hello cli arg config");

    let dst = ctx.dir.join("dst_cli_args");
    let cli_conf = ctx.dir.join("vcopy.conf");
    let mut f = File::create(&cli_conf).unwrap();
    writeln!(f, "# This is a comment").unwrap();
    writeln!(f, "--sources {}", ctx.src.display()).unwrap();
    writeln!(f, "--dest {}", dst.display()).unwrap();
    writeln!(f, "--flatten").unwrap();
    writeln!(f, "--keep-daily 7").unwrap();
    writeln!(f, "--keep-weekly 4").unwrap();
    writeln!(f, "--verbose").unwrap();

    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("-c")
        .arg(&cli_conf)
        .status()
        .unwrap();
    assert!(st.success());
    assert_eq!(
        fs::read(dst.join("cli_arg_file.txt")).unwrap(),
        b"hello cli arg config"
    );
}

#[test]
fn test_config_key_value_and_kebab_case() {
    let ctx = TestContext::new("config_kv");
    ctx.write_src_file("kv_file.txt", b"hello key-value config");

    let dst = ctx.dir.join("dst_kv");
    let kv_conf = ctx.dir.join("vcopy_kv.conf");
    let mut f = File::create(&kv_conf).unwrap();
    writeln!(f, "# Unquoted key-value using exact CLI names").unwrap();
    writeln!(f, "dest = {}", dst.display()).unwrap();
    writeln!(f, "sources = {}", ctx.src.display()).unwrap();
    writeln!(f, "flatten = true").unwrap();
    writeln!(f, "keep-daily = 7").unwrap();
    writeln!(f, "preserve-permissions = true").unwrap();

    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("--config")
        .arg(&kv_conf)
        .status()
        .unwrap();
    assert!(st.success());
    assert_eq!(
        fs::read(dst.join("kv_file.txt")).unwrap(),
        b"hello key-value config"
    );
}

#[test]
fn test_cli_sources_flag() {
    let ctx = TestContext::new("cli_sources_flag");
    ctx.write_src_file("flag_file.txt", b"flag sources content");

    let dst = ctx.dir.join("dst_flag");
    // Test invoking with --sources and --dest
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("--sources")
        .arg(&ctx.src)
        .arg("--dest")
        .arg(&dst)
        .arg("--flatten")
        .status()
        .unwrap();
    assert!(st.success());
    assert_eq!(
        fs::read(dst.join("flag_file.txt")).unwrap(),
        b"flag sources content"
    );
}

#[test]
fn test_manual_embedded_ascii_and_markdown() {
    // 1. Test --manual flag
    let out = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("--manual")
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("VCOPY(1)"));
    assert!(stdout.contains("QUICK START & SIMPLE EXAMPLES"));
    assert!(stdout.contains("Numbered Versions in a Flat Directory (--flatten)"));
    assert!(stdout.contains("Zero-Obfuscation Configuration Files"));

    // 2. Test --manual-markdown flag
    let out_md = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("--manual-markdown")
        .output()
        .unwrap();
    assert!(out_md.status.success());
    let stdout_md = String::from_utf8_lossy(&out_md.stdout);
    assert!(stdout_md.contains("# VCOPY(1) User Manual"));
    assert!(stdout_md.contains("## QUICK START & SIMPLE EXAMPLES"));
    assert!(stdout_md.contains("## PARAMETER & OPTIONS REFERENCE"));

    // 3. Test 'manual' subcommand
    let out_sub = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("manual")
        .output()
        .unwrap();
    assert!(out_sub.status.success());
    let stdout_sub = String::from_utf8_lossy(&out_sub.stdout);
    assert!(stdout_sub.contains("VCOPY(1)"));

    // 4. Test 'manual --markdown' subcommand
    let out_sub_md = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("manual")
        .arg("--markdown")
        .output()
        .unwrap();
    assert!(out_sub_md.status.success());
    let stdout_sub_md = String::from_utf8_lossy(&out_sub_md.stdout);
    assert!(stdout_sub_md.contains("# VCOPY(1) User Manual"));
}
