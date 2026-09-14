use agmux_native::terminal_text::{bounded_terminal_text, cleanup_copied_text};

#[test]
fn copied_terminal_text_normalizes_layout_whitespace() {
    let selection = "  first\u{00a0}line  \r\n  second\t\r\n\r\n";

    assert_eq!(cleanup_copied_text(selection), "first line\nsecond");
}

#[test]
fn copied_terminal_text_preserves_intentional_indentation_after_terminal_padding() {
    let selection = "    indented\n  plain";

    assert_eq!(cleanup_copied_text(selection), "  indented\nplain");
}

#[test]
fn terminal_snapshot_returns_the_requested_tail_and_truncation_metadata() {
    let snapshot = bounded_terminal_text("first  \nsecond\nthird\n\n", 2);

    assert_eq!(snapshot.text, "second\nthird");
    assert_eq!(snapshot.lines, 2);
    assert!(snapshot.is_truncated);
}

#[test]
fn terminal_snapshot_preserves_indentation_and_normalizes_non_breaking_spaces() {
    let snapshot = bounded_terminal_text("  indented\nwith\u{00a0}space", 20);

    assert_eq!(snapshot.text, "  indented\nwith space");
    assert_eq!(snapshot.lines, 2);
    assert!(!snapshot.is_truncated);
}
