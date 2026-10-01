use agtk::history::{InputTracker, history_needle, submitted_prompt};

#[test]
fn input_tracker_emits_a_compacted_prompt_on_submit() {
    let mut tracker = InputTracker::default();

    assert_eq!(tracker.push("review  "), None);
    assert_eq!(tracker.push("this\r"), Some("review this".to_owned()));
}

#[test]
fn input_tracker_handles_terminal_editing_controls() {
    let mut tracker = InputTracker::default();

    tracker.push("mistake\u{7f}");
    tracker.push("en\u{1b}[A");
    assert_eq!(tracker.push("\n"), Some("mistaken".to_owned()));

    tracker.push("discard me");
    tracker.push("\u{15}");
    assert_eq!(tracker.push("keep\r"), Some("keep".to_owned()));
}

#[test]
fn input_tracker_keeps_bracketed_paste_text_without_control_sequences() {
    let mut tracker = InputTracker::default();

    assert_eq!(
        tracker.push("\u{1b}[200~line one\nline two\u{1b}[201~\r"),
        Some("line one line two".to_owned())
    );
}

#[test]
fn history_needle_matches_the_rendered_first_line() {
    assert_eq!(
        history_needle("run  the\tthing\nsecond line", 60),
        "run the thing"
    );
    assert_eq!(
        history_needle("do the long thing...", 60),
        "do the long thing"
    );
}

#[test]
fn history_needle_truncates_at_a_useful_word_boundary() {
    assert_eq!(
        history_needle("one two three four five six", 20),
        "one two three four"
    );
    assert_eq!(history_needle("abcdefghij", 6), "abcdef");
}

#[test]
fn submitted_prompt_keeps_the_hook_text_and_bounds_it() {
    assert_eq!(
        submitted_prompt("  keep\nboth lines  ").as_deref(),
        Some("keep\nboth lines")
    );
    assert_eq!(submitted_prompt(" \n "), None);
    let long = "x".repeat(2_500);
    let bounded = submitted_prompt(&long).expect("long prompts are kept");
    assert_eq!(bounded.chars().count(), 2_001);
    assert!(bounded.ends_with('…'));
}
