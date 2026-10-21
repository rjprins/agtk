use agtk::agent_models::{codex_models, model_options};
use agtk::control::SessionKind;
use serde_json::json;

fn cache() -> String {
    json!({"models": [
        {"slug": "gpt-b", "display_name": "GPT-B", "visibility": "list", "priority": 2,
         "supported_reasoning_levels": [{"effort": "low"}, {"effort": "ultra"}]},
        {"slug": "hidden", "display_name": "Hidden", "visibility": "hide", "priority": 0,
         "supported_reasoning_levels": [{"effort": "minimal"}]},
        {"slug": "gpt-a", "visibility": "list", "priority": 1,
         "supported_reasoning_levels": [{"effort": "high"}, {"effort": "low"}]},
    ]})
    .to_string()
}

#[test]
fn codex_lists_its_picker_models_in_priority_order() {
    let models = codex_models(&cache());
    let summary = models
        .iter()
        .map(|model| {
            (
                model.value.as_str(),
                model.label.as_str(),
                model.efforts.join(","),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        summary,
        [
            ("gpt-a", "gpt-a", "high,low".to_owned()),
            ("gpt-b", "GPT-B", "low,ultra".to_owned()),
        ]
    );
    assert!(codex_models("not json").is_empty());
}

#[test]
fn the_codex_default_offers_every_effort_a_listed_model_has() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("models_cache.json"), cache()).unwrap();
    let options = model_options(SessionKind::Codex, home.path());
    assert_eq!(options[0].value, "");
    assert_eq!(options[0].efforts, ["low", "high", "ultra"]);
    assert_eq!(options.len(), 3);

    // Without a cache only the default is left, with every effort Codex knows.
    let empty = tempfile::tempdir().unwrap();
    let options = model_options(SessionKind::Codex, empty.path());
    assert_eq!(options.len(), 1);
    assert!(options[0].efforts.iter().any(|effort| effort == "medium"));
}

#[test]
fn claude_and_gemini_offer_their_cli_aliases() {
    let nowhere = std::path::Path::new("/nonexistent");
    let claude = model_options(SessionKind::Claude, nowhere);
    assert_eq!(claude[0].value, "");
    assert!(claude.iter().any(|model| model.value == "opus"));
    assert!(
        claude
            .iter()
            .all(|model| model.efforts.contains(&"max".to_owned()))
    );
    let gemini = model_options(SessionKind::Gemini, nowhere);
    assert!(gemini.iter().any(|model| model.value == "flash"));
    assert!(gemini.iter().all(|model| model.efforts.is_empty()));
    assert!(model_options(SessionKind::Shell, nowhere).is_empty());
}
