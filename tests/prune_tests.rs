mod common;
use common::TestContext;
use std::fs::{self, File};
use std::io::Write;
use std::process::Command;
use std::thread;
use std::time::Duration;

#[test]
fn test_count_based_pruning() {
    let tmp = std::env::temp_dir().join(format!(
        "vcopy_test_prune_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    ));
    let _ = fs::remove_dir_all(&tmp);

    let src = tmp.join("src");
    let dst = tmp.join("dst");
    fs::create_dir_all(&src).unwrap();
    let doc = src.join("data.bin");

    for i in 1..=4 {
        let mut f = File::create(&doc).unwrap();
        writeln!(f, "run {}", i).unwrap();
        thread::sleep(Duration::from_millis(1050));

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

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn test_prune_preserves_older_snapshot_restore() {
    let ctx = TestContext::new("prune_integrity");

    // Run 1 (Baseline v1): file1="v1", file2="f2_v1"
    ctx.write_src_file("file1.txt", b"v1 content");
    ctx.write_src_file("file2.txt", b"f2_v1 content");
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("create")
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());

    // Run 2 (Archives S1=v1, live becomes v2): file1 modified to "v2", file2 untouched ("f2_v1")
    ctx.write_src_file("file1.txt", b"v2 content");
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("create")
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());

    // Run 3 (Archives S2=v2, live becomes v3): file1="v3", file2 modified to "f2_v2" (displacing "f2_v1" into S2!)
    ctx.write_src_file("file1.txt", b"v3 content");
    ctx.write_src_file("file2.txt", b"f2_v2 content");
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("create")
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());

    // Run 4 (Archives S3=v3, live becomes v4): file1="v4"
    ctx.write_src_file("file1.txt", b"v4 content");
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("create")
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());

    let hist_dir = ctx.dst.join(".version-history");
    let mut snaps: Vec<_> = fs::read_dir(&hist_dir)
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.len() == 17 && n.contains('_'))
        .collect();
    snaps.sort();
    assert_eq!(snaps.len(), 3);

    // Rename S1 to an earlier hour (e.g. 2020-01-01_010000) so GFS pruning keeps S1 and S3,
    // and prunes the INTERMEDIATE snapshot S2 (where "f2_v1 content" was displaced!).
    let old_s1_name = "2020-01-01_010000";
    fs::rename(hist_dir.join(&snaps[0]), hist_dir.join(old_s1_name)).unwrap();

    // Run GFS prune with keep-hourly=2 (keeps S3 in current hour and old_s1_name in 2020 hour; prunes intermediate S2!)
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("prune")
        .arg("--keep-hourly")
        .arg("2")
        .arg("--keep-daily")
        .arg("2")
        .arg("--keep-weekly")
        .arg("2")
        .arg("--keep-monthly")
        .arg("2")
        .arg("--keep-yearly")
        .arg("2")
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());

    // S2 must have been pruned, leaving old_s1_name and S3
    assert!(hist_dir.join(old_s1_name).is_dir());
    assert!(!hist_dir.join(&snaps[1]).exists());
    assert!(hist_dir.join(&snaps[2]).is_dir());

    // Restore from S1 (old_s1_name): must have both file1="v1 content" AND file2="f2_v1 content"
    // (which had to be preserved from S2 into S1 when S2 was pruned!)
    let restore_s1 = ctx.dir.join("restored_s1");
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("restore")
        .arg("--target-time")
        .arg(old_s1_name)
        .arg(&ctx.dst)
        .arg(&restore_s1)
        .status()
        .unwrap();
    assert!(st.success());
    assert_eq!(
        fs::read_to_string(restore_s1.join("file1.txt")).unwrap(),
        "v1 content"
    );
    assert_eq!(
        fs::read_to_string(restore_s1.join("file2.txt")).unwrap(),
        "f2_v1 content"
    );

    // Restore from S3 (snaps[2]): must have file1="v3 content" and file2="f2_v2 content"
    let restore_s3 = ctx.dir.join("restored_s3");
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("restore")
        .arg("--target-time")
        .arg(&snaps[2])
        .arg(&ctx.dst)
        .arg(&restore_s3)
        .status()
        .unwrap();
    assert!(st.success());
    assert_eq!(
        fs::read_to_string(restore_s3.join("file1.txt")).unwrap(),
        "v3 content"
    );
    assert_eq!(
        fs::read_to_string(restore_s3.join("file2.txt")).unwrap(),
        "f2_v2 content"
    );

    // Restore latest: must have file1="v4 content" and file2="f2_v2 content"
    let restore_latest = ctx.dir.join("restored_latest");
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("restore")
        .arg(&ctx.dst)
        .arg(&restore_latest)
        .status()
        .unwrap();
    assert!(st.success());
    assert_eq!(
        fs::read_to_string(restore_latest.join("file1.txt")).unwrap(),
        "v4 content"
    );
    assert_eq!(
        fs::read_to_string(restore_latest.join("file2.txt")).unwrap(),
        "f2_v2 content"
    );
}

#[test]
fn test_prune_ignores_non_snapshot_dirs_and_symlinks() {
    let ctx = TestContext::new("prune_safety");
    ctx.write_src_file("a.txt", b"v1");
    assert!(Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap()
        .success());

    ctx.write_src_file("a.txt", b"v2");
    assert!(Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap()
        .success());

    // Place a non-snapshot directory and a symlink to an external directory inside .version-history
    let hist_dir = ctx.dst.join(".version-history");
    let custom_dir = hist_dir.join("custom_notes_dir!");
    fs::create_dir_all(&custom_dir).unwrap();
    fs::write(custom_dir.join("keep.txt"), b"important").unwrap();

    let external_dir = ctx.dir.join("external_important");
    fs::create_dir_all(&external_dir).unwrap();
    fs::write(external_dir.join("do_not_delete.txt"), b"critical data").unwrap();

    #[cfg(unix)]
    std::os::unix::fs::symlink(&external_dir, hist_dir.join("2020-01-01_000000")).unwrap();

    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("prune")
        .arg("-n")
        .arg("0")
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());

    // Neither custom_dir nor external_dir may be deleted
    assert!(custom_dir.join("keep.txt").is_file());
    assert!(external_dir.join("do_not_delete.txt").is_file());
}

#[test]
fn test_flat_pruning_and_consolidation() {
    let ctx = TestContext::new("flat_prune");
    ctx.write_src_file("doc.txt", b"v1 content");

    // Run 1 (creates baseline with doc v1)
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("--flatten")
        .arg("create")
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());

    std::thread::sleep(std::time::Duration::from_millis(1050));

    // Run 2 (Archives S1=v1, live becomes v2)
    ctx.write_src_file("doc.txt", b"v2 content");
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("create")
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());

    std::thread::sleep(std::time::Duration::from_millis(1050));

    // Run 3 (Archives S2=v2, live becomes v3)
    ctx.write_src_file("doc.txt", b"v3 content");
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("create")
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());

    let flat_manifest_path = ctx.dst.join(".version-history.baseline.json");
    let flat_json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&flat_manifest_path).unwrap()).unwrap();
    assert_eq!(flat_json["snapshots"].as_array().unwrap().len(), 2);

    // Prune keeping only the latest 1 snapshot (-n 1)
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("prune")
        .arg("-n")
        .arg("1")
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());

    let flat_json_after: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&flat_manifest_path).unwrap()).unwrap();
    assert_eq!(flat_json_after["snapshots"].as_array().unwrap().len(), 1);

    // Surviving snapshot must restore successfully
    let restore_out = ctx.dir.join("restored_surviving");
    let surviving_snap_name = flat_json_after["snapshots"][0]["snapshot_name"]
        .as_str()
        .unwrap();
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("restore")
        .arg("--target-time")
        .arg(surviving_snap_name)
        .arg(&ctx.dst)
        .arg(&restore_out)
        .status()
        .unwrap();
    assert!(st.success());
    assert_eq!(
        fs::read(restore_out.join("doc.txt")).unwrap(),
        b"v2 content"
    );
}
