use std::fs;
use std::os::unix::fs::PermissionsExt;

use agtk::rebuild::RebuildPlan;

fn script(path: &std::path::Path, contents: &str) {
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn rebuild_uses_the_running_target_and_handles_spaces_and_deleted_binary() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source tree");
    fs::create_dir_all(root.join("scripts")).unwrap();
    fs::write(root.join("Cargo.toml"), "[package]\nname = 'agtk'\n").unwrap();
    script(
        &root.join("scripts/build-viewer.sh"),
        "#!/bin/sh\ntouch viewer-built\n",
    );
    let target = temp.path().join("custom target");
    fs::create_dir_all(target.join("debug")).unwrap();
    let binary = target.join("debug/agtk");
    script(&binary, "#!/bin/sh\nexit 0\n");
    let cargo = temp.path().join("fake cargo");
    script(
        &cargo,
        "#!/bin/sh\n[ -f viewer-built ] || exit 1\nprintf '%s\\n' \"$@\" > cargo-args\n",
    );
    let plan = RebuildPlan::new(&root, &target.join("debug/agtk (deleted)")).unwrap();
    assert_eq!(plan.run(&cargo).unwrap(), binary);
    assert_eq!(
        fs::read_to_string(root.join("cargo-args")).unwrap(),
        format!(
            "build\n--locked\n--bins\n--target-dir\n{}\n",
            target.display()
        )
    );
}

#[test]
fn failed_viewer_or_rust_build_does_not_request_a_restart() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join("scripts")).unwrap();
    fs::write(root.join("Cargo.toml"), "[package]\nname = 'agtk'\n").unwrap();
    fs::create_dir_all(root.join("target/debug")).unwrap();
    let cargo = root.join("cargo");
    script(
        &cargo,
        "#!/bin/sh\ntouch cargo-ran\necho compile-failed >&2\nexit 1\n",
    );
    let viewer = root.join("scripts/build-viewer.sh");
    script(&viewer, "#!/bin/sh\necho viewer-failed >&2\nexit 1\n");
    let plan = RebuildPlan::new(root, &root.join("target/debug/agtk")).unwrap();
    assert!(plan.run(&cargo).unwrap_err().contains("viewer-failed"));
    assert!(!root.join("cargo-ran").exists());
    script(&viewer, "#!/bin/sh\nexit 0\n");
    assert!(plan.run(&cargo).unwrap_err().contains("compile-failed"));
    script(&cargo, "#!/bin/sh\nexit 0\n");
    assert!(plan.run(&cargo).unwrap_err().contains("agtk"));
}
