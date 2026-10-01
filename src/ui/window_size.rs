use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(super) const WINDOW_SIZE_PREFERENCE: &str = "windowSize";
const DEFAULT_WIDTH: i32 = 1200;
const DEFAULT_HEIGHT: i32 = 800;
// Smaller stored sizes are treated as corrupt rather than opening an unusable window.
const MIN_WIDTH: i32 = 480;
const MIN_HEIGHT: i32 = 320;

/// The main window's size while not maximized, and whether it was maximized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WindowSize {
    pub width: i32,
    pub height: i32,
    #[serde(default)]
    pub maximized: bool,
}

impl Default for WindowSize {
    fn default() -> Self {
        Self {
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            maximized: false,
        }
    }
}

impl WindowSize {
    pub(super) fn from_preference(value: Option<&Value>) -> Self {
        value
            .and_then(|value| serde_json::from_value::<Self>(value.clone()).ok())
            .filter(|size| size.width >= MIN_WIDTH && size.height >= MIN_HEIGHT)
            .unwrap_or_default()
    }

    pub(super) fn to_preference(self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::WindowSize;
    use serde_json::json;

    #[test]
    fn missing_size_uses_the_default() {
        assert_eq!(WindowSize::from_preference(None), WindowSize::default());
    }

    #[test]
    fn stored_size_is_restored_with_its_maximized_state() {
        let size = WindowSize::from_preference(Some(&json!({
            "width": 1600, "height": 1000, "maximized": true
        })));
        assert_eq!(
            size,
            WindowSize {
                width: 1600,
                height: 1000,
                maximized: true
            }
        );
    }

    #[test]
    fn unusable_stored_size_uses_the_default() {
        let tiny = WindowSize::from_preference(Some(&json!({"width": 10, "height": 10})));
        assert_eq!(tiny, WindowSize::default());
        let broken = WindowSize::from_preference(Some(&json!("wide")));
        assert_eq!(broken, WindowSize::default());
    }

    #[test]
    fn preference_round_trips() {
        let size = WindowSize {
            width: 1400,
            height: 900,
            maximized: false,
        };
        assert_eq!(
            WindowSize::from_preference(Some(&size.to_preference())),
            size
        );
    }
}
