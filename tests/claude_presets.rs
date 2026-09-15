use agmux_native::claude_presets::{ClaudeEffort, ClaudeModelPreset, ClaudePresetPreferences};
use serde_json::json;

#[test]
fn defaults_are_immediately_useful_and_generate_exact_claude_commands() {
    let preferences = ClaudePresetPreferences::default();
    assert_eq!(preferences.presets.len(), 2);
    assert_eq!(
        preferences.presets[0].commands(),
        ["/model sonnet\r", "/effort auto\r"]
    );
    assert_eq!(
        preferences.presets[1].commands(),
        ["/model opus\r", "/effort high\r"]
    );
}

#[test]
fn validation_rejects_duplicates_whitespace_and_control_characters() {
    let preset = ClaudeModelPreset {
        id: "focused".to_owned(),
        name: "Focused".to_owned(),
        model: "opus".to_owned(),
        effort: ClaudeEffort::High,
    };
    assert!(ClaudePresetPreferences::new(vec![preset.clone()]).is_ok());
    assert!(ClaudePresetPreferences::new(vec![preset.clone(), preset]).is_err());
    assert!(
        ClaudePresetPreferences::new(vec![ClaudeModelPreset {
            id: "bad id".to_owned(),
            name: "Bad".to_owned(),
            model: "opus".to_owned(),
            effort: ClaudeEffort::High,
        }])
        .is_err()
    );
    assert!(
        ClaudePresetPreferences::new(vec![ClaudeModelPreset {
            id: "bad-model".to_owned(),
            name: "Bad".to_owned(),
            model: "opus high".to_owned(),
            effort: ClaudeEffort::High,
        }])
        .is_err()
    );
}

#[test]
fn malformed_persisted_items_are_ignored_without_losing_valid_presets() {
    let preferences = ClaudePresetPreferences::from_value_lossy(json!([
        {"id":"valid","name":"Valid","model":"sonnet","effort":"xhigh"},
        {"id":"invalid","name":"Invalid","model":"two words","effort":"high"},
        {"id":"valid","name":"Duplicate","model":"opus","effort":"max"}
    ]));
    assert_eq!(preferences.presets.len(), 1);
    assert_eq!(preferences.presets[0].id, "valid");
    assert_eq!(preferences.presets[0].effort, ClaudeEffort::Xhigh);
}
