use agmux_native::shortcuts::{ShortcutAction, ShortcutPreferences};

#[test]
fn shortcut_defaults_preserve_the_approved_keyboard_contract() {
    let preferences = ShortcutPreferences::default();

    assert_eq!(
        preferences.binding(ShortcutAction::NewShell),
        "<Control><Shift>grave"
    );
    assert_eq!(
        preferences.binding(ShortcutAction::ToggleSidebar),
        "<Control><Shift>backslash"
    );
    assert_eq!(
        preferences.binding(ShortcutAction::NextSession),
        "<Control><Shift>bracketright"
    );
    assert_eq!(
        preferences.binding(ShortcutAction::LastSession),
        "<Control><Shift>l"
    );
    assert_eq!(
        preferences.binding(ShortcutAction::IncreaseFontSize),
        "<Control>plus"
    );
    assert_eq!(
        preferences.binding(ShortcutAction::DecreaseFontSize),
        "<Control>minus"
    );
    assert_eq!(preferences.bindings.len(), ShortcutAction::ALL.len());
}

#[test]
fn shortcut_preferences_have_stable_serialized_action_keys() {
    let preferences = ShortcutPreferences::default();
    let value = serde_json::to_value(&preferences).expect("serialize shortcuts");

    assert_eq!(value["bindings"]["new-shell"], "<Control><Shift>grave");
    assert_eq!(value["bindings"]["reopen-pr-list"], "<Alt><Shift>p");
    assert_eq!(value["bindings"]["last-session"], "<Control><Shift>l");
    assert_eq!(value["bindings"]["increase-font-size"], "<Control>plus");
    assert_eq!(
        serde_json::from_value::<ShortcutPreferences>(value).expect("deserialize shortcuts"),
        preferences
    );
}
