use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;

use agtk::instance::{InstanceName, InstancePaths, ensure_private_dir};

#[test]
fn instance_paths_are_namespaced_below_explicit_roots() {
    let name = InstanceName::parse("test-123").expect("valid instance name");
    let paths = InstancePaths::new(
        name,
        Path::new("/run/user/1000"),
        Path::new("/home/rutger/.local/state"),
    );

    assert_eq!(
        paths.control_socket(),
        Path::new("/run/user/1000/agtk/test-123/control.sock")
    );
    assert_eq!(
        paths.sessions_dir(),
        Path::new("/run/user/1000/agtk/test-123/sessions")
    );
    assert_eq!(
        paths.captures_dir(),
        Path::new("/run/user/1000/agtk/test-123/captures")
    );
    assert_eq!(
        paths.database(),
        Path::new("/home/rutger/.local/state/agtk/test-123/agtk.db")
    );
}

#[test]
fn instance_names_reject_path_and_dbus_metacharacters() {
    for invalid in ["", ".", "../live", "test/one", "test one", "test.one"] {
        assert!(
            InstanceName::parse(invalid).is_err(),
            "accepted invalid instance {invalid:?}"
        );
    }
}

#[test]
fn instance_names_produce_distinct_valid_application_ids() {
    let default = InstanceName::parse("default").expect("default instance");
    let test = InstanceName::parse("test-123").expect("test instance");

    assert_eq!(default.application_id(), "nl.rutger.Agtk");
    assert_eq!(test.application_id(), "nl.rutger.Agtk.Devel.i_test_123");
    assert_ne!(default.application_id(), test.application_id());
}

#[test]
fn private_dirs_are_created_owner_only_and_tightened() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("agtk/default");
    ensure_private_dir(&dir).unwrap();
    assert_eq!(
        fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o700
    );

    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    ensure_private_dir(&dir).unwrap();
    assert_eq!(
        fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
}

#[test]
fn private_dirs_refuse_symlinks_and_parents_others_can_write() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("elsewhere");
    fs::create_dir(&target).unwrap();
    let link = root.path().join("link");
    symlink(&target, &link).unwrap();
    assert!(ensure_private_dir(&link).is_err());

    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(ensure_private_dir(&shared.join("agtk")).is_err());

    fs::set_permissions(&shared, fs::Permissions::from_mode(0o1777)).unwrap();
    ensure_private_dir(&shared.join("agtk")).unwrap();
}
