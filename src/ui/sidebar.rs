use serde_json::Value;

pub(super) const SIDEBAR_WIDTH_PREFERENCE: &str = "sidebarWidth";
pub(super) const DEFAULT_SIDEBAR_WIDTH: i32 = 320;
pub(super) const MIN_SIDEBAR_WIDTH: i32 = 280;
pub(super) const MAX_SIDEBAR_WIDTH: i32 = 800;

pub(super) const CHANGES_WIDTH_PREFERENCE: &str = "changesSidebarWidth";
pub(super) const DEFAULT_CHANGES_WIDTH: i32 = 360;
pub(super) const MIN_CHANGES_WIDTH: i32 = 240;
pub(super) const MAX_CHANGES_WIDTH: i32 = 800;

pub(super) fn sidebar_width(value: Option<&Value>) -> i32 {
    stored_width(
        value,
        DEFAULT_SIDEBAR_WIDTH,
        MIN_SIDEBAR_WIDTH,
        MAX_SIDEBAR_WIDTH,
    )
}

pub(super) fn changes_width(value: Option<&Value>) -> i32 {
    stored_width(
        value,
        DEFAULT_CHANGES_WIDTH,
        MIN_CHANGES_WIDTH,
        MAX_CHANGES_WIDTH,
    )
}

fn stored_width(value: Option<&Value>, default: i32, min: i32, max: i32) -> i32 {
    value
        .and_then(Value::as_i64)
        .and_then(|width| i32::try_from(width).ok())
        .filter(|width| *width > 0)
        .map(|width| width.clamp(min, max))
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::{changes_width, sidebar_width};

    #[test]
    fn missing_sidebar_width_uses_broader_default() {
        assert_eq!(sidebar_width(None), 320);
    }

    #[test]
    fn stored_sidebar_width_is_clamped_to_safe_bounds() {
        assert_eq!(sidebar_width(Some(&serde_json::json!(100))), 280);
        assert_eq!(sidebar_width(Some(&serde_json::json!(480))), 480);
        assert_eq!(sidebar_width(Some(&serde_json::json!(2_000))), 800);
    }

    #[test]
    fn invalid_stored_sidebar_width_uses_the_default() {
        assert_eq!(sidebar_width(Some(&serde_json::json!("wide"))), 320);
        assert_eq!(sidebar_width(Some(&serde_json::json!(-1))), 320);
    }

    #[test]
    fn stored_changes_width_is_clamped_to_its_own_bounds() {
        assert_eq!(changes_width(None), 360);
        assert_eq!(changes_width(Some(&serde_json::json!(100))), 240);
        assert_eq!(changes_width(Some(&serde_json::json!(520))), 520);
        assert_eq!(changes_width(Some(&serde_json::json!(2_000))), 800);
    }
}
