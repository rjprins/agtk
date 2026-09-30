use agtk::projects::ProjectPreferences;

#[test]
fn project_preferences_persist_pin_and_collapse_independently() {
    let mut preferences = ProjectPreferences::default();
    preferences.set("/work/agtk", Some(true), None);
    preferences.set("/work/agtk", None, Some(true));

    let project = preferences.get("/work/agtk");
    assert!(project.is_pinned);
    assert!(project.is_collapsed);
    assert!(!preferences.get("/work/other").is_pinned);

    let json = serde_json::to_value(&preferences).expect("serialize projects");
    assert_eq!(json["projects"]["/work/agtk"]["isPinned"], true);
    assert_eq!(
        serde_json::from_value::<ProjectPreferences>(json).expect("deserialize projects"),
        preferences
    );
}
