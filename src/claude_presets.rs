use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use crate::AppResult as ClaudePresetResult;
const MAX_PRESETS: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClaudeEffort {
    Auto,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
    Ultracode,
}

impl ClaudeEffort {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
            Self::Ultracode => "ultracode",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaudeModelPreset {
    pub id: String,
    pub name: String,
    pub model: String,
    pub effort: ClaudeEffort,
}

impl ClaudeModelPreset {
    pub fn commands(&self) -> [String; 2] {
        [
            format!("/model {}\r", self.model),
            format!("/effort {}\r", self.effort.as_str()),
        ]
    }

    fn validate(&self) -> ClaudePresetResult<()> {
        let valid_id = (1..=80).contains(&self.id.len())
            && self
                .id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte));
        if !valid_id {
            return Err("preset ID contains unsupported characters".into());
        }
        if self.name.trim() != self.name
            || !(1..=80).contains(&self.name.chars().count())
            || self.name.chars().any(char::is_control)
        {
            return Err("preset name must be 1 to 80 printable characters".into());
        }
        if self.model.trim() != self.model
            || !(1..=200).contains(&self.model.chars().count())
            || self
                .model
                .chars()
                .any(|character| character.is_whitespace() || character.is_control())
        {
            return Err("preset model must be one printable terminal argument".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaudePresetPreferences {
    pub presets: Vec<ClaudeModelPreset>,
}

impl ClaudePresetPreferences {
    pub fn new(presets: Vec<ClaudeModelPreset>) -> ClaudePresetResult<Self> {
        if presets.len() > MAX_PRESETS {
            return Err("no more than 50 Claude presets are allowed".into());
        }
        let mut ids = BTreeSet::new();
        for preset in &presets {
            preset.validate()?;
            if !ids.insert(&preset.id) {
                return Err(format!("duplicate Claude preset ID: {}", preset.id).into());
            }
        }
        Ok(Self { presets })
    }

    pub fn from_value_lossy(value: Value) -> Self {
        let candidates = value.as_array().cloned().unwrap_or_default();
        let mut presets = Vec::new();
        let mut ids = BTreeSet::new();
        for candidate in candidates.into_iter().take(MAX_PRESETS) {
            let Ok(preset) = serde_json::from_value::<ClaudeModelPreset>(candidate) else {
                continue;
            };
            if preset.validate().is_ok() && ids.insert(preset.id.clone()) {
                presets.push(preset);
            }
        }
        Self { presets }
    }

    pub fn preset(&self, id: &str) -> Option<&ClaudeModelPreset> {
        self.presets.iter().find(|preset| preset.id == id)
    }
}

impl Default for ClaudePresetPreferences {
    fn default() -> Self {
        Self {
            presets: vec![
                ClaudeModelPreset {
                    id: "sonnet-auto".to_owned(),
                    name: "Sonnet / auto".to_owned(),
                    model: "sonnet".to_owned(),
                    effort: ClaudeEffort::Auto,
                },
                ClaudeModelPreset {
                    id: "opus-high".to_owned(),
                    name: "Opus / high".to_owned(),
                    model: "opus".to_owned(),
                    effort: ClaudeEffort::High,
                },
            ],
        }
    }
}
