use agmux_native::appearance::{AppearancePreferences, ThemeKey, resolve_system_theme, theme};

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
    assert!(!preferences.follow_system);
    assert_eq!(preferences.font, "Monospace 11");
    let json = serde_json::to_value(&preferences).unwrap();
    assert_eq!(
        serde_json::from_value::<AppearancePreferences>(json).unwrap(),
        preferences
    );
}
