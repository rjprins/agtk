use serde_json::Value;

pub(super) const SIDEBAR_WIDTH_PREFERENCE: &str = "sidebarWidth";
pub(super) const DEFAULT_SIDEBAR_WIDTH: i32 = 320;
pub(super) const MIN_SIDEBAR_WIDTH: i32 = 280;
pub(super) const MAX_SIDEBAR_WIDTH: i32 = 800;

pub(super) fn sidebar_width(value: Option<&Value>) -> i32 {
    value
        .and_then(Value::as_i64)
        .and_then(|width| i32::try_from(width).ok())
        .filter(|width| *width > 0)
        .map(|width| width.clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH))
        .unwrap_or(DEFAULT_SIDEBAR_WIDTH)
}

#[cfg(test)]
mod tests {
    use super::sidebar_width;

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
}
