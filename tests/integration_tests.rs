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
    assert!(
        stdout.is_ascii(),
        "ASCII manual output must be pure 7-bit ASCII"
    );
    assert!(stdout.contains("VCOPY(1) User Manual\n===================="));
    assert!(stdout.contains("QUICK START & SIMPLE EXAMPLES\n-----------------------------"));
    assert!(stdout.contains("Numbered Versions in a Flat Directory"));
    assert!(stdout.contains("Zero-Obfuscation Configuration Files"));
    assert!(!stdout.contains("```"));
    assert!(!stdout.contains("**"));
    assert!(!stdout.contains("$N$"));

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

#[test]
fn test_manifest_metadata_comment() {
    let ctx = TestContext::new("manifest_comment");
    ctx.write_src_file("doc.txt", b"hello manifest comment");

    // 1. Run hierarchical backup
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());

    // Check baseline.json has _comment near the top
    let baseline_path = ctx.dst.join(".version-history").join("baseline.json");
    let baseline_raw = fs::read_to_string(&baseline_path).unwrap();
    assert!(baseline_raw.contains("\"_comment\": \"Created and managed by vcopy"));
    assert!(baseline_raw.contains("vcopy restore"));

    // Check that _comment is among the first lines
    let first_lines: Vec<&str> = baseline_raw.lines().take(4).collect();
    let first_joined = first_lines.join("\n");
    assert!(first_joined.contains("\"_comment\""));

    // 2. Modify and trigger historical snapshot manifest.json
    thread::sleep(Duration::from_millis(1050));
    ctx.write_src_file("doc.txt", b"hello manifest comment v2");
    let st2 = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st2.success());

    let snapshots: Vec<_> = fs::read_dir(ctx.dst.join(".version-history"))
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_dir())
        .collect();
    assert_eq!(snapshots.len(), 1);
    let snap_manifest_raw = fs::read_to_string(snapshots[0].path().join("manifest.json")).unwrap();
    assert!(snap_manifest_raw.contains("\"_comment\": \"Created and managed by vcopy"));

    // 3. Test flat mode manifest comment
    let flat_dst = ctx.dir.join("dst_flat");
    let st3 = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("--flatten")
        .arg(&ctx.src)
        .arg(&flat_dst)
        .status()
        .unwrap();
    assert!(st3.success());

    let flat_baseline_raw =
        fs::read_to_string(flat_dst.join(".version-history.baseline.json")).unwrap();
    assert!(flat_baseline_raw.contains("\"_comment\": \"Created and managed by vcopy"));
    let flat_first_lines: Vec<&str> = flat_baseline_raw.lines().take(4).collect();
    assert!(flat_first_lines.join("\n").contains("\"_comment\""));
}

#[test]
fn test_orphan_scan_recovery_flat_mode() {
    let ctx = TestContext::new("orphan_flat");
    ctx.write_src_file("doc1.txt", b"doc 1 original v1");
    ctx.write_src_file("doc2.txt", b"doc 2 original v1");

    // 1. Initial backup in flat mode
    let st1 = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("--flatten")
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st1.success());

    thread::sleep(Duration::from_millis(1050));
    // 2. Modify doc1 to create .v1 file in flat root
    ctx.write_src_file("doc1.txt", b"doc 1 updated v2");
    let st2 = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st2.success());

    assert!(ctx.dst.join("doc1.v1.txt").is_file());
    assert_eq!(
        fs::read(ctx.dst.join("doc1.v1.txt")).unwrap(),
        b"doc 1 original v1"
    );
    assert_eq!(
        fs::read(ctx.dst.join("doc1.txt")).unwrap(),
        b"doc 1 updated v2"
    );

    // 3. Intentionally DESTROY all metadata files
    let baseline_json = ctx.dst.join(".version-history.baseline.json");
    let manifest_jsonl = ctx.dst.join(".version-history.manifest.jsonl");
    assert!(baseline_json.is_file());
    assert!(manifest_jsonl.is_file());
    fs::remove_file(&baseline_json).unwrap();
    fs::remove_file(&manifest_jsonl).unwrap();

    // 4. Verify vcopy list auto-recovers the flat manifest from orphan .v1 files
    let list_out = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("list")
        .arg(&ctx.dst)
        .output()
        .unwrap();
    assert!(list_out.status.success());
    let list_stdout = String::from_utf8_lossy(&list_out.stdout);
    assert!(
        list_stdout.contains("Snapshot (flat)"),
        "list output should detect flat repo and show header: {}",
        list_stdout
    );
    assert!(
        baseline_json.is_file(),
        "baseline.json should be automatically reconstructed"
    );
    assert!(
        manifest_jsonl.is_file(),
        "manifest.jsonl should be automatically reconstructed"
    );

    // 5. Verify restore latest works from recovered manifest
    let restore_latest_out = ctx.dir.join("restored_latest");
    let st_rest = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("restore")
        .arg(&ctx.dst)
        .arg(&restore_latest_out)
        .status()
        .unwrap();
    assert!(st_rest.success());
    assert_eq!(
        fs::read(restore_latest_out.join("doc1.txt")).unwrap(),
        b"doc 1 updated v2"
    );
    assert_eq!(
        fs::read(restore_latest_out.join("doc2.txt")).unwrap(),
        b"doc 2 original v1"
    );

    // 6. Verify subsequent backup continues monotonic version numbering (.v2, NOT clobbering .v1)
    thread::sleep(Duration::from_millis(1050));
    ctx.write_src_file("doc1.txt", b"doc 1 updated v3");
    let st3 = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("--max-versions")
        .arg("10")
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st3.success());

    assert!(ctx.dst.join("doc1.v1.txt").is_file());
    assert_eq!(
        fs::read(ctx.dst.join("doc1.v1.txt")).unwrap(),
        b"doc 1 original v1"
    );
    assert!(
        ctx.dst.join("doc1.v2.txt").is_file(),
        "new version should be .v2, not colliding with .v1"
    );
    assert_eq!(
        fs::read(ctx.dst.join("doc1.v2.txt")).unwrap(),
        b"doc 1 updated v2"
    );
    assert_eq!(
        fs::read(ctx.dst.join("doc1.txt")).unwrap(),
        b"doc 1 updated v3"
    );
}

#[test]
fn test_orphan_scan_recovery_hierarchical_mode() {
    let ctx = TestContext::new("orphan_hierarchical");
    ctx.write_src_file("file1.txt", b"hierarchical file 1 v1");
    ctx.write_src_file("to_delete.txt", b"hierarchical file to delete");

    // 1. Initial backup
    let st1 = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st1.success());

    thread::sleep(Duration::from_millis(1050));
    // 2. Modify file1 and delete to_delete.txt
    ctx.write_src_file("file1.txt", b"hierarchical file 1 v2");
    fs::remove_file(ctx.src.join("to_delete.txt")).unwrap();

    let st2 = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st2.success());

    let hist_dir = ctx.dst.join(".version-history");
    let snapshots: Vec<_> = fs::read_dir(&hist_dir)
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_dir())
        .collect();
    assert_eq!(snapshots.len(), 1);

    let snap_dir = &snapshots[0].path();
    let snap_name = snapshots[0].file_name().to_string_lossy().to_string();
    assert!(snap_dir.join("file1.txt").is_file());
    assert!(snap_dir.join("to_delete.txt.deleted").is_file());
    assert!(snap_dir.join("manifest.json").is_file());
    assert!(hist_dir.join("baseline.json").is_file());

    // 3. Intentionally DESTROY manifest.json and baseline.json
    fs::remove_file(snap_dir.join("manifest.json")).unwrap();
    fs::remove_file(hist_dir.join("baseline.json")).unwrap();

    // 4. Verify vcopy list auto-reconstructs manifest.json and baseline.json
    let list_out = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("list")
        .arg(&ctx.dst)
        .output()
        .unwrap();
    assert!(list_out.status.success());
    let list_stdout = String::from_utf8_lossy(&list_out.stdout);
    assert!(
        list_stdout.contains(&snap_name),
        "list stdout should contain snapshot name: {}",
        list_stdout
    );
    assert!(
        snap_dir.join("manifest.json").is_file(),
        "manifest.json should be auto-reconstructed"
    );
    assert!(
        hist_dir.join("baseline.json").is_file(),
        "baseline.json should be auto-reconstructed"
    );

    // Verify reconstructed manifest accurately captured displaced file and deleted marker
    let reconstructed_manifest_raw = fs::read_to_string(snap_dir.join("manifest.json")).unwrap();
    let reconstructed_json: serde_json::Value =
        serde_json::from_str(&reconstructed_manifest_raw).unwrap();
    assert_eq!(reconstructed_json["summary"]["files_modified"], 1);
    assert_eq!(reconstructed_json["summary"]["files_deleted"], 1);

    // 5. Verify point-in-time restore works from recovered snapshot
    let pit_out = ctx.dir.join("restored_pit");
    let st_pit = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("restore")
        .arg("--target-time")
        .arg(&snap_name)
        .arg(&ctx.dst)
        .arg(&pit_out)
        .status()
        .unwrap();
    assert!(st_pit.success());
    assert_eq!(
        fs::read(pit_out.join("file1.txt")).unwrap(),
        b"hierarchical file 1 v1"
    );

    // 6. Verify subsequent backup proceeds seamlessly against the recovered baseline
    thread::sleep(Duration::from_millis(1050));
    ctx.write_src_file("file1.txt", b"hierarchical file 1 v3");
    let st3 = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st3.success());
    assert_eq!(
        fs::read(ctx.dst.join("file1.txt")).unwrap(),
        b"hierarchical file 1 v3"
    );
}

#[test]
fn test_total_wipeout_circuit_breaker() {
    let ctx = TestContext::new("circuit_breaker");
    ctx.write_src_file("file1.txt", b"first file");
    ctx.write_src_file("file2.txt", b"second file");

    // 1. Initial backup
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());
    assert!(ctx.dst.join("file1.txt").exists());
    assert!(ctx.dst.join("file2.txt").exists());

    // 2. Remove all source files (simulating unmounted / cleared drive, 100% loss)
    fs::remove_file(ctx.src.join("file1.txt")).unwrap();
    fs::remove_file(ctx.src.join("file2.txt")).unwrap();

    // 3. Attempt sync - must be blocked by safety circuit breaker
    let output = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Safety circuit breaker") || stderr.contains("100%"),
        "stderr should mention circuit breaker: {}",
        stderr
    );

    // Destination files must remain intact
    assert!(ctx.dst.join("file1.txt").exists());
    assert!(ctx.dst.join("file2.txt").exists());

    // 4. Run with --allow-empty-source - should succeed and archive/displace
    let st_allowed = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("--allow-empty-source")
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st_allowed.success());
    assert!(!ctx.dst.join("file1.txt").exists());
    assert!(!ctx.dst.join("file2.txt").exists());

    // Verify files were archived in snapshot
    let hist_dir = ctx.dst.join(".version-history");
    assert!(hist_dir.is_dir());
}

#[test]
fn test_missing_source_path_protection() {
    let ctx = TestContext::new("missing_src");
    let missing_src = ctx.dir.join("nonexistent_directory");

    // Attempt backup with missing source path
    let output = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&missing_src)
        .arg(&ctx.dst)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("does not exist or is unavailable"),
        "stderr should mention missing source: {}",
        stderr
    );

    // With --force, the missing source check is bypassed
    let st_forced = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("--force")
        .arg(&missing_src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st_forced.success());
}

#[test]
fn test_destination_safety_guards() {
    let ctx = TestContext::new("dest_guards");
    ctx.write_src_file("sample.txt", b"sample content");

    // 1. Refuse system directory (/etc or /usr)
    let output_etc = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg("/etc")
        .output()
        .unwrap();
    assert!(!output_etc.status.success());
    let stderr_etc = String::from_utf8_lossy(&output_etc.stderr);
    assert!(
        stderr_etc.contains("refusing to use system or root directory"),
        "stderr: {}",
        stderr_etc
    );

    // 2. Refuse foreign non-empty directory without --allow-existing
    let foreign_dir = ctx.dir.join("foreign_dir");
    fs::create_dir_all(&foreign_dir).unwrap();
    fs::write(foreign_dir.join("unrelated_file.doc"), b"do not touch").unwrap();

    let output_foreign = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&foreign_dir)
        .output()
        .unwrap();
    assert!(!output_foreign.status.success());
    let stderr_foreign = String::from_utf8_lossy(&output_foreign.stderr);
    assert!(
        stderr_foreign.contains("contains non-vcopy files, but has no vcopy repository metadata"),
        "stderr: {}",
        stderr_foreign
    );

    // With --allow-existing, backup succeeds into the existing directory
    let st_foreign_allowed = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("--allow-existing")
        .arg(&ctx.src)
        .arg(&foreign_dir)
        .status()
        .unwrap();
    assert!(st_foreign_allowed.success());
    assert!(foreign_dir.join("sample.txt").exists());
    // In hierarchical sync, destination mirrors source; foreign unrelated file is safely displaced to .version-history/
    assert!(foreign_dir.join(".version-history").is_dir());
}

#[test]
fn test_shorter_cli_flags_and_aliases() {
    let ctx = TestContext::new("shorter_flags");
    ctx.write_src_file("alpha.txt", b"content alpha");

    // 1. Test --flat (shortened for --flatten)
    let st_flat = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("--flat")
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st_flat.success());
    assert!(ctx.dst.join("alpha.txt").exists());
    assert!(ctx.dst.join(".version-history.manifest.jsonl").exists());

    // 2. Test 'ls' subcommand alias for 'list'
    let output_ls = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("ls")
        .arg(&ctx.dst)
        .output()
        .unwrap();
    assert!(output_ls.status.success());
    let stdout_ls = String::from_utf8_lossy(&output_ls.stdout);
    assert!(
        stdout_ls.contains("Snapshot (flat)"),
        "stdout should contain flat header: {}",
        stdout_ls
    );

    // 3. Test restore with short flag -t and alias 'snapshot'
    let restore_out = ctx.dir.join("restore_out");
    let st_restore = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("restore")
        .arg("-t")
        .arg("latest")
        .arg(&ctx.dst)
        .arg(&restore_out)
        .status()
        .unwrap();
    assert!(st_restore.success());
    assert_eq!(
        fs::read(restore_out.join("alpha.txt")).unwrap(),
        b"content alpha"
    );

    // 4. Test retention aliases (--hourly, --daily, --weekly, --monthly, --yearly)
    let st_prune = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("prune")
        .arg("--hourly")
        .arg("10")
        .arg("--daily")
        .arg("5")
        .arg("--weekly")
        .arg("2")
        .arg("--monthly")
        .arg("6")
        .arg("--yearly")
        .arg("1")
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st_prune.success());
}

#[test]
fn test_exclude_system() {
    let ctx = TestContext::new("exclude_system");

    // Create source files:
    // keep.txt
    // temp.tmp (should be excluded by --exclude *.tmp)
    // node_modules/package.json (should be excluded by --exclude node_modules/)
    // ignored_by_file.log (should be excluded by --exclude-from)
    // ignored_by_vcopyignore.bak (should be excluded by .vcopyignore)
    // docs/guide.md (should be kept)
    // docs/draft.tmp (should be excluded by *.tmp)

    fs::write(ctx.src.join("keep.txt"), b"keep me").unwrap();
    fs::write(ctx.src.join("temp.tmp"), b"temp data").unwrap();
    fs::create_dir_all(ctx.src.join("node_modules")).unwrap();
    fs::write(ctx.src.join("node_modules/package.json"), b"pkg").unwrap();

    let ignore_file = ctx.dir.join("my_ignores.txt");
    fs::write(&ignore_file, b"*.log\n# comment\n").unwrap();
    fs::write(ctx.src.join("ignored_by_file.log"), b"log data").unwrap();

    fs::write(ctx.src.join(".vcopyignore"), b"*.bak\n").unwrap();
    fs::write(ctx.src.join("ignored_by_vcopyignore.bak"), b"bak data").unwrap();

    fs::create_dir_all(ctx.src.join("docs")).unwrap();
    fs::write(ctx.src.join("docs/guide.md"), b"guide").unwrap();
    fs::write(ctx.src.join("docs/draft.tmp"), b"draft").unwrap();

    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("-e")
        .arg("*.tmp")
        .arg("--exclude")
        .arg("node_modules/")
        .arg("--exclude-from")
        .arg(&ignore_file)
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());

    // Kept files should exist in dst
    assert!(ctx.dst.join("keep.txt").exists());
    assert!(ctx.dst.join("docs/guide.md").exists());

    // Excluded files should NOT exist in dst
    assert!(!ctx.dst.join("temp.tmp").exists());
    assert!(!ctx.dst.join("node_modules").exists());
    assert!(!ctx.dst.join("node_modules/package.json").exists());
    assert!(!ctx.dst.join("ignored_by_file.log").exists());
    assert!(!ctx.dst.join("ignored_by_vcopyignore.bak").exists());
    assert!(!ctx.dst.join("docs/draft.tmp").exists());

    // Verify baseline records the excludes
    let baseline_content =
        fs::read_to_string(ctx.dst.join(".version-history/baseline.json")).unwrap();
    assert!(baseline_content.contains("*.tmp"));
    assert!(baseline_content.contains("node_modules/"));
    assert!(baseline_content.contains("*.log"));
}

#[test]
fn test_verify_and_bit_rot_detection() {
    let ctx = TestContext::new("verify_bitrot");

    fs::write(ctx.src.join("file1.txt"), b"initial content 1").unwrap();
    fs::write(ctx.src.join("file2.txt"), b"initial content 2").unwrap();

    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());

    // 1. Initial verify should pass
    let st_verify = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("verify")
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st_verify.success());

    // 2. Modify file1.txt (creating a snapshot with file1 displaced and file2 untouched in live mirror)
    std::thread::sleep(std::time::Duration::from_millis(1050));
    fs::write(ctx.src.join("file1.txt"), b"updated content 1").unwrap();
    let st2 = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st2.success());

    // Multi-snapshot verify (both full and --quick) must succeed
    let st_verify_multi = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("verify")
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st_verify_multi.success());

    let st_check = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("check")
        .arg("--quick")
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st_check.success());

    // 3. Corrupt file1.txt by mutating bytes of same length (simulating bit-rot)
    fs::write(ctx.dst.join("file1.txt"), b"mutated content 1").unwrap();

    let out_corrupt = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("verify")
        .arg(&ctx.dst)
        .output()
        .unwrap();
    assert!(
        !out_corrupt.status.success(),
        "verify should fail on corrupted file"
    );
    let stderr_str = String::from_utf8_lossy(&out_corrupt.stderr);
    let stdout_str = String::from_utf8_lossy(&out_corrupt.stdout);
    let combined = format!("{}\n{}", stdout_str, stderr_str);
    assert!(
        combined.contains("[CORRUPTED]") || combined.contains("BIT-ROT DETECTED"),
        "Output should report corruption: {}",
        combined
    );

    // 4. Restore file1 and delete file2 (simulate missing file)
    fs::write(ctx.dst.join("file1.txt"), b"updated content 1").unwrap();
    fs::remove_file(ctx.dst.join("file2.txt")).unwrap();

    let out_missing = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("verify")
        .arg(&ctx.dst)
        .output()
        .unwrap();
    assert!(
        !out_missing.status.success(),
        "verify should fail on missing file"
    );
    let stdout_missing = String::from_utf8_lossy(&out_missing.stdout);
    assert!(
        stdout_missing.contains("[MISSING]"),
        "Output should report missing file: {}",
        stdout_missing
    );

    // 5. Also verify multi-snapshot flat mode repository
    let flat_dst = ctx.dir.join("flat_dst");
    fs::create_dir_all(&flat_dst).unwrap();
    let st_flat1 = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("--flat")
        .arg(&ctx.src)
        .arg(&flat_dst)
        .status()
        .unwrap();
    assert!(st_flat1.success());

    std::thread::sleep(std::time::Duration::from_millis(1050));
    fs::write(ctx.src.join("file1.txt"), b"flat v2 content 1").unwrap();
    let st_flat2 = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("--flat")
        .arg(&ctx.src)
        .arg(&flat_dst)
        .status()
        .unwrap();
    assert!(st_flat2.success());

    let st_verify_flat = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("verify")
        .arg("-v")
        .arg(&flat_dst)
        .status()
        .unwrap();
    assert!(st_verify_flat.success());

    // Corrupt flat archive file (file1.v1.txt) and add an untracked file
    fs::write(flat_dst.join("file1.v1.txt"), b"corrupted flat v1").unwrap();
    fs::write(flat_dst.join("stray_untracked.bin"), b"orphan").unwrap();

    let out_flat_corrupt = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("verify")
        .arg("-v")
        .arg(&flat_dst)
        .output()
        .unwrap();
    assert!(!out_flat_corrupt.status.success());
    let flat_stdout = String::from_utf8_lossy(&out_flat_corrupt.stdout);
    assert!(flat_stdout.contains("[CORRUPTED]"));
    assert!(flat_stdout.contains("[UNTRACKED] stray_untracked.bin"));

    // Verify with non-matching -t timestamp or empty directory fails
    let out_bad_time = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("verify")
        .arg("-t")
        .arg("1999-01-01")
        .arg(&flat_dst)
        .output()
        .unwrap();
    assert!(!out_bad_time.status.success());

    let empty_dir = ctx.dir.join("empty_not_vcopy");
    fs::create_dir_all(&empty_dir).unwrap();
    let out_empty = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("verify")
        .arg(&empty_dir)
        .output()
        .unwrap();
    assert!(!out_empty.status.success());
}

#[test]
fn test_flat_date_naming_and_orphan_recovery() {
    let ctx = TestContext::new("flat_date");

    // Version 1
    fs::write(ctx.src.join("notes.txt"), b"notes version 1").unwrap();

    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("--flat")
        .arg("--flat-date")
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());
    assert_eq!(
        fs::read(ctx.dst.join("notes.txt")).unwrap(),
        b"notes version 1"
    );

    std::thread::sleep(std::time::Duration::from_millis(1050));

    // Version 2 (modifies notes.txt)
    fs::write(ctx.src.join("notes.txt"), b"notes version 2").unwrap();

    let st2 = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("--flat")
        .arg("--flat-date")
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st2.success());
    assert_eq!(
        fs::read(ctx.dst.join("notes.txt")).unwrap(),
        b"notes version 2"
    );

    // Check that a timestamped archive file was created (e.g. notes.2026-*.txt)
    let entries: Vec<_> = fs::read_dir(&ctx.dst)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with("notes.20") && n.ends_with(".txt"))
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "Expected 1 timestamped archive file, found: {:?}",
        entries
    );

    // Test orphan scan recovery by removing metadata files
    let baseline_json = ctx.dst.join(".version-history.baseline.json");
    let manifest_jsonl = ctx.dst.join(".version-history.manifest.jsonl");
    assert!(baseline_json.is_file());
    assert!(manifest_jsonl.is_file());
    fs::remove_file(&baseline_json).unwrap();
    fs::remove_file(&manifest_jsonl).unwrap();

    // vcopy ls should trigger orphan scan recovery
    let ls_out = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("ls")
        .arg(&ctx.dst)
        .output()
        .unwrap();
    assert!(ls_out.status.success());
    let ls_stdout = String::from_utf8_lossy(&ls_out.stdout);
    assert!(ls_stdout.contains("Snapshot (flat)"));

    // Manifests should be reconstructed
    assert!(baseline_json.is_file());
    assert!(manifest_jsonl.is_file());

    // Restore version 1
    let restore_out = ctx.dir.join("restore_out");
    let st_restore = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("restore")
        .arg("-t")
        .arg(&entries[0][6..23]) // extract timestamp
        .arg(&ctx.dst)
        .arg(&restore_out)
        .status()
        .unwrap();
    assert!(st_restore.success());
    assert_eq!(
        fs::read(restore_out.join("notes.txt")).unwrap(),
        b"notes version 1"
    );
}

#[test]
fn test_self_healing_missing_destination_file() {
    let ctx = TestContext::new("self_healing");
    ctx.write_src_file("vital.txt", b"vital data");

    // 1. Hierarchical mode self-healing
    assert!(Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap()
        .success());
    assert!(ctx.dst.join("vital.txt").is_file());

    // Accidentally delete vital.txt from destination mirror while source is unchanged
    fs::remove_file(ctx.dst.join("vital.txt")).unwrap();
    assert!(!ctx.dst.join("vital.txt").exists());

    // Re-running vcopy must detect the missing destination file and self-heal (restored_missing)
    let out_heal = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("-v")
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .output()
        .unwrap();
    assert!(out_heal.status.success());
    let stdout_heal = String::from_utf8_lossy(&out_heal.stdout);
    assert!(
        stdout_heal.contains("+ vital.txt"),
        "Expected self-healing '+ vital.txt' log, got: {}",
        stdout_heal
    );
    let baseline_raw = fs::read_to_string(ctx.dst.join(".version-history/baseline.json")).unwrap();
    assert!(
        baseline_raw.contains("restored_missing"),
        "Expected baseline.json to record restored_missing action"
    );
    assert_eq!(fs::read(ctx.dst.join("vital.txt")).unwrap(), b"vital data");

    // 2. Flat mode self-healing
    let flat_dst = ctx.dir.join("flat_dst");
    assert!(Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("--flat")
        .arg(&ctx.src)
        .arg(&flat_dst)
        .status()
        .unwrap()
        .success());
    fs::remove_file(flat_dst.join("vital.txt")).unwrap();

    let out_flat_heal = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("--flat")
        .arg("-v")
        .arg(&ctx.src)
        .arg(&flat_dst)
        .output()
        .unwrap();
    assert!(out_flat_heal.status.success());
    assert_eq!(fs::read(flat_dst.join("vital.txt")).unwrap(), b"vital data");
}

#[test]
fn test_nested_vcopyignore_and_config_subcommands() {
    let ctx = TestContext::new("nested_ignore_cfg");

    fs::create_dir_all(ctx.src.join("sub/deep")).unwrap();
    fs::create_dir_all(ctx.src.join("sub/cache")).unwrap();
    fs::write(
        ctx.src.join("sub/.vcopyignore"),
        b"*.tmp\n/anchored.txt\ncache/\n",
    )
    .unwrap();
    fs::write(ctx.src.join("sub/keep.txt"), b"keep sub").unwrap();
    fs::write(ctx.src.join("sub/skip.tmp"), b"skip 1").unwrap();
    fs::write(ctx.src.join("sub/deep/skip2.tmp"), b"skip 2").unwrap();
    fs::write(ctx.src.join("sub/anchored.txt"), b"skip anchored").unwrap();
    fs::write(ctx.src.join("sub/deep/anchored.txt"), b"keep deep anchored").unwrap();
    fs::write(ctx.src.join("sub/cache/item.bin"), b"skip cache").unwrap();

    assert!(Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap()
        .success());

    assert!(ctx.dst.join("sub/keep.txt").is_file());
    assert!(ctx.dst.join("sub/deep/anchored.txt").is_file());
    assert!(!ctx.dst.join("sub/skip.tmp").exists());
    assert!(!ctx.dst.join("sub/deep/skip2.tmp").exists());
    assert!(!ctx.dst.join("sub/anchored.txt").exists());
    assert!(!ctx.dst.join("sub/cache").exists());

    // Test config-driven `verify --quick`
    let verify_cfg = ctx.dir.join("verify.conf");
    fs::write(
        &verify_cfg,
        format!("verify --quick --dest \"{}\"\n", ctx.dst.display()),
    )
    .unwrap();
    assert!(Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("-c")
        .arg(&verify_cfg)
        .status()
        .unwrap()
        .success());

    // Test config-driven `restore` with `include` filter
    let restore_out = ctx.dir.join("cfg_restore_out");
    let restore_cfg = ctx.dir.join("restore.toml");
    fs::write(
        &restore_cfg,
        format!(
            "command = \"restore\"\ndest = '{}'\ntarget-out = '{}'\ninclude = \"keep.txt\"\n",
            ctx.dst.display(),
            restore_out.display()
        ),
    )
    .unwrap();
    assert!(Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("-c")
        .arg(&restore_cfg)
        .status()
        .unwrap()
        .success());
    assert!(restore_out.join("sub/keep.txt").is_file());
    assert!(!restore_out.join("sub/deep/anchored.txt").exists());
}

#[test]
fn test_stale_pid_lock_recovery() {
    let ctx = TestContext::new("stale_pid_lock");
    ctx.write_src_file("doc.txt", b"hello");

    let hist_dir = ctx.dst.join(".version-history");
    fs::create_dir_all(&hist_dir).unwrap();
    // Write a stale PID that cannot exist (999999999)
    fs::write(hist_dir.join(".pid_lock"), b"999999999\n").unwrap();

    // vcopy should clean up any stale .pid_lock if encountered and complete backup
    assert!(Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap()
        .success());
    assert!(ctx.dst.join("doc.txt").is_file());
}
