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

/// The positions that change when `dragged` lands before or after `target`.
///
/// `group` is one sidebar group in display order. The group keeps its own
/// position values, only handed out in the new order, so the global ordering
/// used by shortcuts stays intact for the other groups.
pub(super) fn reordered_positions(
    group: &[(String, i64)],
    dragged: &str,
    target: &str,
    before: bool,
) -> Vec<(String, i64)> {
    if dragged == target {
        return Vec::new();
    }
    let mut ids = group.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>();
    let Some(from) = ids.iter().position(|id| *id == dragged) else {
        return Vec::new();
    };
    ids.remove(from);
    let Some(to) = ids.iter().position(|id| *id == target) else {
        return Vec::new();
    };
    ids.insert(if before { to } else { to + 1 }, dragged);

    // Discovered sessions can share a position; make the slots strictly increasing.
    let mut slots = group
        .iter()
        .map(|(_, position)| *position)
        .collect::<Vec<_>>();
    slots.sort_unstable();
    for index in 1..slots.len() {
        slots[index] = slots[index].max(slots[index - 1] + 1);
    }
    ids.into_iter()
        .zip(slots)
        .map(|(id, position)| (id.to_owned(), position))
        .filter(|(id, position)| {
            group
                .iter()
                .any(|(old_id, old_position)| old_id == id && old_position != position)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{changes_width, reordered_positions, sidebar_width};

    fn group(ids: &[(&str, i64)]) -> Vec<(String, i64)> {
        ids.iter()
            .map(|(id, position)| ((*id).to_owned(), *position))
            .collect()
    }

    #[test]
    fn dragging_a_session_down_shifts_the_rows_it_passes() {
        let moved = reordered_positions(&group(&[("a", 3), ("b", 5), ("c", 9)]), "a", "c", false);
        assert_eq!(moved, group(&[("b", 3), ("c", 5), ("a", 9)]));
    }

    #[test]
    fn dropping_above_a_row_lands_before_it() {
        let moved = reordered_positions(&group(&[("a", 0), ("b", 1), ("c", 2)]), "c", "a", true);
        assert_eq!(moved, group(&[("c", 0), ("a", 1), ("b", 2)]));
    }

    #[test]
    fn dropping_a_row_where_it_already_is_changes_nothing() {
        let rows = group(&[("a", 0), ("b", 1), ("c", 2)]);
        assert!(reordered_positions(&rows, "b", "b", true).is_empty());
        assert!(reordered_positions(&rows, "b", "a", false).is_empty());
        assert!(reordered_positions(&rows, "b", "c", true).is_empty());
        assert!(reordered_positions(&rows, "zz", "c", true).is_empty());
    }

    #[test]
    fn duplicate_positions_are_spread_out() {
        let moved = reordered_positions(&group(&[("a", 2), ("b", 2), ("c", 2)]), "c", "a", true);
        assert_eq!(moved, group(&[("a", 3), ("b", 4)]));
    }

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
