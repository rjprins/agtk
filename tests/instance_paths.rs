use std::path::Path;

use agmux_native::instance::{InstanceName, InstancePaths};

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
        Path::new("/run/user/1000/agmux-native/test-123/control.sock")
    );
    assert_eq!(
        paths.sessions_dir(),
        Path::new("/run/user/1000/agmux-native/test-123/sessions")
    );
    assert_eq!(
        paths.captures_dir(),
        Path::new("/run/user/1000/agmux-native/test-123/captures")
    );
    assert_eq!(
        paths.database(),
        Path::new("/home/rutger/.local/state/agmux-native/test-123/agmux.db")
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

    assert_eq!(default.application_id(), "nl.rutger.AgmuxNative");
    assert_eq!(
        test.application_id(),
        "nl.rutger.AgmuxNative.Devel.i_test_123"
    );
    assert_ne!(default.application_id(), test.application_id());
}
