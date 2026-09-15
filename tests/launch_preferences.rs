use agmux_native::control::SessionKind;
use agmux_native::launch_preferences::QuickLaunchPreferences;

#[test]
fn quick_launch_preferences_round_trip_project_context() {
    let preferences = QuickLaunchPreferences {
        kind: SessionKind::Codex,
        args: vec!["--model".to_owned(), "gpt-5".to_owned()],
        cwd: Some("/work/agmux-feature".into()),
        project_root: Some("/work/agmux".into()),
        worktree_path: Some("/work/agmux-feature".into()),
    };

    let value = serde_json::to_value(&preferences).expect("serialize launch preferences");
    assert_eq!(value["kind"], "codex");
    assert_eq!(value["args"], serde_json::json!(["--model", "gpt-5"]));
    assert_eq!(value["projectRoot"], "/work/agmux");
    assert_eq!(
        serde_json::from_value::<QuickLaunchPreferences>(value).expect("deserialize preferences"),
        preferences
    );
}
