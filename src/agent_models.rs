//! The models and reasoning efforts each agent CLI accepts at startup.

use std::path::Path;

use serde::Deserialize;

use crate::session::SessionKind;

/// Effort levels in the order the pickers show them.
const EFFORT_ORDER: &[&str] = &["minimal", "low", "medium", "high", "xhigh", "max", "ultra"];
const CLAUDE_EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

/// Each agent with its model options.
pub type AgentModels = Vec<(SessionKind, Vec<ModelOption>)>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelOption {
    /// What `--model` gets. Empty leaves the model to the agent's own config.
    pub value: String,
    pub label: String,
    pub efforts: Vec<String>,
}

impl ModelOption {
    fn new(value: &str, label: &str, efforts: &[&str]) -> Self {
        Self {
            value: value.to_owned(),
            label: label.to_owned(),
            efforts: efforts.iter().copied().map(str::to_owned).collect(),
        }
    }
}

/// The agent's models, led by its own default. Claude and Gemini get the
/// aliases their CLIs resolve to the latest model; Codex lists what its model
/// cache says the signed-in account may pick.
pub fn model_options(kind: SessionKind, codex_home: &Path) -> Vec<ModelOption> {
    match kind {
        SessionKind::Claude => {
            let mut options = vec![ModelOption::new("", "Agent default", CLAUDE_EFFORTS)];
            options.extend(
                [
                    ("fable", "Fable"),
                    ("fable[1m]", "Fable (1M context)"),
                    ("opus", "Opus"),
                    ("opus[1m]", "Opus (1M context)"),
                    ("sonnet", "Sonnet"),
                    ("sonnet[1m]", "Sonnet (1M context)"),
                    ("haiku", "Haiku"),
                    ("opusplan", "Opus to plan, Sonnet to build"),
                ]
                .map(|(value, label)| ModelOption::new(value, label, CLAUDE_EFFORTS)),
            );
            options
        }
        SessionKind::Codex => {
            let models = std::fs::read_to_string(codex_home.join("models_cache.json"))
                .map(|json| codex_models(&json))
                .unwrap_or_default();
            let mut options = vec![ModelOption {
                value: String::new(),
                label: "Agent default".to_owned(),
                efforts: if models.is_empty() {
                    EFFORT_ORDER.iter().copied().map(str::to_owned).collect()
                } else {
                    known_efforts(&models)
                },
            }];
            options.extend(models);
            options
        }
        SessionKind::Gemini => [
            ("", "Agent default"),
            ("auto", "Auto"),
            ("pro", "Pro"),
            ("flash", "Flash"),
            ("flash-lite", "Flash Lite"),
        ]
        .map(|(value, label)| ModelOption::new(value, label, &[]))
        .to_vec(),
        SessionKind::Shell | SessionKind::Custom => Vec::new(),
    }
}

/// The models Codex shows in its own picker, in its order.
pub fn codex_models(cache_json: &str) -> Vec<ModelOption> {
    #[derive(Deserialize)]
    struct Cache {
        models: Vec<Model>,
    }
    #[derive(Deserialize)]
    struct Model {
        slug: String,
        display_name: Option<String>,
        visibility: Option<String>,
        #[serde(default)]
        priority: i64,
        #[serde(default)]
        supported_reasoning_levels: Vec<Level>,
    }
    #[derive(Deserialize)]
    struct Level {
        effort: String,
    }
    let Ok(cache) = serde_json::from_str::<Cache>(cache_json) else {
        return Vec::new();
    };
    let mut models = cache
        .models
        .into_iter()
        .filter(|model| model.visibility.as_deref() == Some("list"))
        .collect::<Vec<_>>();
    models.sort_by_key(|model| model.priority);
    models
        .into_iter()
        .map(|model| ModelOption {
            label: model.display_name.unwrap_or_else(|| model.slug.clone()),
            value: model.slug,
            efforts: model
                .supported_reasoning_levels
                .into_iter()
                .map(|level| level.effort)
                .collect(),
        })
        .collect()
}

/// Every effort some model supports, so the agent's default model can use any of them.
fn known_efforts(models: &[ModelOption]) -> Vec<String> {
    let mut efforts = EFFORT_ORDER
        .iter()
        .filter(|effort| {
            models
                .iter()
                .any(|model| model.efforts.iter().any(|e| e == *effort))
        })
        .map(|effort| (*effort).to_owned())
        .collect::<Vec<_>>();
    for model in models {
        for effort in &model.efforts {
            if !efforts.contains(effort) {
                efforts.push(effort.clone());
            }
        }
    }
    efforts
}
