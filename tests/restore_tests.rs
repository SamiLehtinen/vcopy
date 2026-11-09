mod common;
use common::TestContext;
use std::fs;
use std::process::Command;
use std::thread;
use std::time::Duration;

#[test]
fn test_point_in_time_restore() {
    let ctx = TestContext::new("restore");
    let restore_out = ctx.dir.join("restored_target");

    // Version 1: 2 files
    ctx.write_src_file("doc.txt", b"version 1");
    ctx.write_src_file("unmodified.txt", b"never changed");

    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());

    thread::sleep(Duration::from_millis(1050));

    // Version 2: doc.txt modified, added.txt added, unmodified.txt untouched
    ctx.write_src_file("doc.txt", b"version 2");
    ctx.write_src_file("added.txt", b"version 2 extra");

    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());

    // Obtain the first snapshot identifier from destination
    let hist_dir = ctx.dst.join(".version-history");
    let mut snaps: Vec<_> = fs::read_dir(&hist_dir)
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.len() == 17 && n.contains('_'))
        .collect();
    snaps.sort();
    assert!(
        !snaps.is_empty(),
        "Expected at least one historical snapshot"
    );
    let first_snap = snaps.first().unwrap().clone();

    // Run restore targeting the timestamp of the first snapshot
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("restore")
        .arg("--target-time")
        .arg(&first_snap)
        .arg(&ctx.dst)
        .arg(&restore_out)
        .status()
        .unwrap();
    assert!(st.success());

    // Restored directory must reflect state at snapshot 1
    assert!(restore_out.join("doc.txt").is_file());
    assert_eq!(
        fs::read_to_string(restore_out.join("doc.txt")).unwrap(),
        "version 1"
    );

    // Unmodified file must be correctly restored
    assert!(restore_out.join("unmodified.txt").is_file());
    assert_eq!(
        fs::read_to_string(restore_out.join("unmodified.txt")).unwrap(),
        "never changed"
    );

    // File added only in snapshot 2 must NOT be in restored snapshot 1
    assert!(!restore_out.join("added.txt").exists());
}

#[test]
fn test_multi_snapshot_restore_with_later_deletions_and_modifications() {
    let ctx = TestContext::new("multi_restore");

    // Version 1: a="a1", b="b1", del="del1"
    ctx.write_src_file("a.txt", b"a1");
    ctx.write_src_file("b.txt", b"b1");
    ctx.write_src_file("del.txt", b"del1");
    assert!(Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("create")
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap()
        .success());

    // Version 2: a="a2", b untouched ("b1"), del untouched ("del1"), c="c2" added
    ctx.write_src_file("a.txt", b"a2");
    ctx.write_src_file("c.txt", b"c2");
    assert!(Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("create")
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap()
        .success());

    // Version 3: b modified to "b3", del.txt deleted!
    ctx.write_src_file("b.txt", b"b3");
    fs::remove_file(ctx.src.join("del.txt")).unwrap();
    assert!(Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("create")
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap()
        .success());

    let hist_dir = ctx.dst.join(".version-history");
    let mut snaps: Vec<_> = fs::read_dir(&hist_dir)
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.len() == 17 && n.contains('_'))
        .collect();
    snaps.sort();
    assert_eq!(snaps.len(), 2); // snaps[0] = v1, snaps[1] = v2, live = v3

    // Restore snaps[0] (v1): even though b.txt and del.txt were only modified/deleted in v3 (snaps[1]),
    // restoring snaps[0] must find them in snaps[1]!
    let out_v1 = ctx.dir.join("out_v1");
    assert!(Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("restore")
        .arg("--target-time")
        .arg(&snaps[0])
        .arg(&ctx.dst)
        .arg(&out_v1)
        .status()
        .unwrap()
        .success());
    assert_eq!(fs::read_to_string(out_v1.join("a.txt")).unwrap(), "a1");
    assert_eq!(fs::read_to_string(out_v1.join("b.txt")).unwrap(), "b1");
    assert_eq!(fs::read_to_string(out_v1.join("del.txt")).unwrap(), "del1");
    assert!(!out_v1.join("c.txt").exists());

    // Restore snaps[1] (v2): a="a2", b="b1", del="del1", c="c2"
    let out_v2 = ctx.dir.join("out_v2");
    assert!(Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("restore")
        .arg("--target-time")
        .arg(&snaps[1])
        .arg(&ctx.dst)
        .arg(&out_v2)
        .status()
        .unwrap()
        .success());
    assert_eq!(fs::read_to_string(out_v2.join("a.txt")).unwrap(), "a2");
    assert_eq!(fs::read_to_string(out_v2.join("b.txt")).unwrap(), "b1");
    assert_eq!(fs::read_to_string(out_v2.join("del.txt")).unwrap(), "del1");
    assert_eq!(fs::read_to_string(out_v2.join("c.txt")).unwrap(), "c2");
}

#[test]
fn test_restore_latest() {
    let ctx = TestContext::new("restore_latest");
    let restore_out = ctx.dir.join("restored_latest");

    ctx.write_src_file("hello.txt", b"hello world");

    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());

    thread::sleep(Duration::from_millis(1050));
    ctx.write_src_file("hello.txt", b"hello world v2");

    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());

    // Restore latest
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("restore")
        .arg(&ctx.dst)
        .arg(&restore_out)
        .status()
        .unwrap();
    assert!(st.success());

    assert_eq!(
        fs::read_to_string(restore_out.join("hello.txt")).unwrap(),
        "hello world v2"
    );
}

#[test]
fn test_restore_permissions_option() {
    let ctx = TestContext::new("perms_opt");

    ctx.write_src_file("script.sh", b"#!/bin/sh\necho hello\n");
    ctx.write_src_file("readonly.txt", b"locked content\n");
    let script_path = ctx.src.join("script.sh");
    let ro_path = ctx.src.join("readonly.txt");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms).unwrap();

        let mut ro_perms = fs::metadata(&ro_path).unwrap().permissions();
        ro_perms.set_mode(0o444);
        fs::set_permissions(&ro_path, ro_perms).unwrap();
    }

    // 1. Initial backup with `create -p` (testing subcommand flag placement too)
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("create")
        .arg("-p")
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());

    // Verify the backup mirror copy on disk was NOT locked down with 0o444 permissions
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mirror_ro_mode = fs::metadata(ctx.dst.join("readonly.txt"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_ne!(
            mirror_ro_mode, 0o444,
            "Mirror copy should not inherit restrictive OS permissions"
        );
        assert!(
            (mirror_ro_mode & 0o200) != 0,
            "Mirror copy should remain owner-writable"
        );
    }

    // Verify baseline.json contains mode 0o755 (493) and 0o444 (292)
    let baseline_path = ctx.dst.join(".version-history").join("baseline.json");
    let content = fs::read_to_string(&baseline_path).unwrap();
    #[cfg(unix)]
    {
        assert!(content.contains("\"mode\": 493")); // 0o755
        assert!(content.contains("\"mode\": 292")); // 0o444
    }

    // 2. Permission-only change on script.sh (0o755 -> 0o640) without changing content or mtime
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o640);
        fs::set_permissions(&script_path, perms).unwrap();
    }

    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("-p")
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());

    // Snapshot 1 should now be archived in .version-history
    let hist_dir = ctx.dst.join(".version-history");
    let mut snaps: Vec<_> = fs::read_dir(&hist_dir)
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.len() == 17 && n.contains('_'))
        .collect();
    snaps.sort();
    assert_eq!(snaps.len(), 1);

    // 3. Restore historical snapshot 1 with -p (should restore 0o755 and 0o444)
    let restore_snap1 = ctx.dir.join("restored_snap1");
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("restore")
        .arg("-p")
        .arg("--target-time")
        .arg(&snaps[0])
        .arg(&ctx.dst)
        .arg(&restore_snap1)
        .status()
        .unwrap();
    assert!(st.success());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let snap1_script_mode = fs::metadata(restore_snap1.join("script.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(snap1_script_mode, 0o755);

        let snap1_ro_mode = fs::metadata(restore_snap1.join("readonly.txt"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(snap1_ro_mode, 0o444);
    }

    // 4. Restore latest with -p (should restore 0o640 on script.sh)
    let restore_latest = ctx.dir.join("restored_latest");
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("restore")
        .arg("-p")
        .arg(&ctx.dst)
        .arg(&restore_latest)
        .status()
        .unwrap();
    assert!(st.success());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let latest_script_mode = fs::metadata(restore_latest.join("script.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(latest_script_mode, 0o640);
    }
}
