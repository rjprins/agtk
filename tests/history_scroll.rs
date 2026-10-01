//! Scrolling the terminal back to a submitted prompt, against a real VTE buffer.
//! AGTK_TEST_DISPLAY=/absolute/path/to/wayland-socket cargo test --test history_scroll -- --ignored

use std::time::{Duration, Instant};

use gtk::prelude::*;
use vte::prelude::*;

use agtk::ui::scroll_to_prompt;

fn pump_until(condition: impl Fn() -> bool) {
    let context = gtk::glib::MainContext::default();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the terminal"
        );
        context.iteration(false);
    }
}

fn terminal_with_lines(lines: &[String]) -> vte::Terminal {
    let terminal = vte::Terminal::new();
    terminal.set_scrollback_lines(-1);
    // VTE only processes fed bytes once the widget is realized.
    let window = gtk::Window::new();
    window.set_default_size(800, 480);
    window.set_child(Some(&terminal));
    window.present();
    let text = lines.join("\r\n") + "\r\n";
    terminal.feed(text.as_bytes());
    // VTE processes fed bytes from an idle handler; the scrollbar grows as rows land.
    let rows = lines.len() as f64;
    pump_until(|| {
        terminal
            .vadjustment()
            .is_some_and(|adjustment| adjustment.upper() >= rows)
    });
    terminal
}

/// Asserts that `row` is on screen; VTE shows a backward match on the bottom row.
fn assert_shows_row(terminal: &vte::Terminal, row: i64) {
    // Let VTE apply the pending scroll before reading the adjustment.
    let context = gtk::glib::MainContext::default();
    let deadline = Instant::now() + Duration::from_millis(200);
    while Instant::now() < deadline {
        context.iteration(false);
    }
    let adjustment = terminal.vadjustment().expect("terminal has a scrollbar");
    let first = adjustment.value() as i64;
    let last = first + adjustment.page_size() as i64 - 1;
    assert!(
        (first..=last).contains(&row),
        "row {row} is not within the visible rows {first}..={last}"
    );
}

#[test]
#[ignore = "requires AGTK_TEST_DISPLAY private Wayland compositor"]
fn prompts_are_found_from_the_end_even_after_a_failed_search() {
    let display = std::env::var("AGTK_TEST_DISPLAY").expect("private display required");
    // SAFETY: the test process is single-threaded at this point.
    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", display);
        std::env::set_var("GDK_BACKEND", "wayland");
    }
    gtk::init().expect("gtk init");

    let mut lines = (0..300).map(|i| format!("line {i}")).collect::<Vec<_>>();
    lines[20] = "❯ first prompt here".to_owned();
    lines[100] = "❯ continue".to_owned();
    lines[200] = "❯ continue".to_owned();
    lines[250] = "❯ last one".to_owned();
    let terminal = terminal_with_lines(&lines);
    let history = [
        "first prompt here".to_owned(),
        "continue".to_owned(),
        "continue".to_owned(),
        "last one".to_owned(),
        "never shown".to_owned(),
    ];

    assert!(scroll_to_prompt(&terminal, &history, 3));
    assert_shows_row(&terminal, 250);
    // A prompt that fell out of scrollback must not break later searches.
    assert!(!scroll_to_prompt(&terminal, &history, 4));
    assert!(scroll_to_prompt(&terminal, &history, 0));
    assert_shows_row(&terminal, 20);
    // Repeated prompts resolve to their own occurrence.
    assert!(scroll_to_prompt(&terminal, &history, 2));
    assert_shows_row(&terminal, 200);
    assert!(scroll_to_prompt(&terminal, &history, 1));
    assert_shows_row(&terminal, 100);
    // The newest prompt is found again after an older one was selected.
    assert!(scroll_to_prompt(&terminal, &history, 3));
    assert_shows_row(&terminal, 250);
}
