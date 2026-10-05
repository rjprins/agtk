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

#[test]
fn a_remembered_project_stays_listed_until_it_is_removed() {
    let mut preferences = ProjectPreferences::default();
    assert!(preferences.remember("/work/agtk"));
    assert!(!preferences.remember("/work/agtk"));

    preferences.set("/work/agtk", Some(true), Some(true));
    preferences.set("/work/agtk", Some(false), Some(false));
    assert!(preferences.projects.contains_key("/work/agtk"));

    assert!(preferences.remove("/work/agtk"));
    assert!(!preferences.remove("/work/agtk"));
    assert!(preferences.projects.is_empty());
}

#[test]
fn a_remembered_project_saves_in_the_settings_format_older_builds_read() {
    let mut preferences = ProjectPreferences::default();
    preferences.remember("/work/agtk");
    assert_eq!(
        serde_json::to_value(&preferences).expect("serialize projects"),
        serde_json::json!({"projects":{"/work/agtk":{"isPinned":false,"isCollapsed":false}}})
    );
}
