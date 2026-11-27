use agtk::appearance::{AppearancePreferences, ThemeKey, resolve_system_theme, theme};

#[test]
fn registry_preserves_every_original_agmux_theme() {
    assert_eq!(
        ThemeKey::ALL.map(ThemeKey::as_str),
        [
            "neutral",
            "neutral-light",
            "dracula",
            "tokyo-night",
            "solarized-dark",
            "solarized-light",
            "light",
        ]
    );
    for key in ThemeKey::ALL {
        let palette = theme(key);
        assert_eq!(palette.terminal.ansi.len(), 16);
        assert!(!palette.name.is_empty());
        assert!(palette.terminal.background.starts_with('#'));
    }
}

#[test]
fn system_theme_pairings_match_the_original_application() {
    assert_eq!(
        resolve_system_theme(ThemeKey::Neutral, false),
        ThemeKey::NeutralLight
    );
    assert_eq!(
        resolve_system_theme(ThemeKey::Neutral, true),
        ThemeKey::Neutral
    );
    assert_eq!(
        resolve_system_theme(ThemeKey::Dracula, false),
        ThemeKey::Light
    );
    assert_eq!(
        resolve_system_theme(ThemeKey::TokyoNight, false),
        ThemeKey::Light
    );
    assert_eq!(
        resolve_system_theme(ThemeKey::SolarizedDark, false),
        ThemeKey::SolarizedLight
    );
    assert_eq!(
        resolve_system_theme(ThemeKey::NeutralLight, true),
        ThemeKey::Neutral
    );
}

#[test]
fn appearance_preferences_have_safe_stable_defaults() {
    let preferences = AppearancePreferences::default();
    assert_eq!(preferences.theme, ThemeKey::Neutral);
    assert!(preferences.follow_system);
    assert_eq!(preferences.font, "Monospace 11");
    assert_eq!(preferences.ui_font_size, 13);
    assert_eq!(
        serde_json::to_value(&preferences).unwrap()["viewerFontSize"],
        11
    );
    let json = serde_json::to_value(&preferences).unwrap();
    assert_eq!(
        serde_json::from_value::<AppearancePreferences>(json).unwrap(),
        preferences
    );
}

#[test]
fn older_appearance_preferences_receive_default_ui_and_viewer_sizes() {
    let preferences: AppearancePreferences = serde_json::from_value(serde_json::json!({
        "theme": "neutral",
        "followSystem": false,
        "font": "Monospace 11"
    }))
    .expect("deserialize legacy appearance preferences");
    assert_eq!(preferences.ui_font_size, 13);
    assert_eq!(
        serde_json::to_value(&preferences).unwrap()["viewerFontSize"],
        11
    );
}

#[test]
fn the_pinned_panel_has_no_font_size_of_its_own_until_one_is_set() {
    assert_eq!(AppearancePreferences::default().pinned_font_size, None);
    let saved_before: AppearancePreferences = serde_json::from_value(serde_json::json!({
        "theme": "neutral",
        "followSystem": false,
        "font": "Monospace 11",
        "uiFontSize": 13,
        "viewerFontSize": 11
    }))
    .expect("deserialize preferences saved before the pinned panel");
    assert_eq!(saved_before.pinned_font_size, None);

    let preferences = AppearancePreferences {
        pinned_font_size: Some(9),
        ..AppearancePreferences::default()
    };
    let json = serde_json::to_value(&preferences).unwrap();
    assert_eq!(json["pinnedFontSize"], 9);
    assert_eq!(
        serde_json::from_value::<AppearancePreferences>(json).unwrap(),
        preferences
    );
}
