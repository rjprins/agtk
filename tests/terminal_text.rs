use agmux_native::terminal_text::cleanup_copied_text;

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
