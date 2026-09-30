use std::time::{Duration, Instant};

use agtk::agent_status::{ScreenTracker, Signal, apply_hook, resolve, rules_for};
use agtk::control::{SessionKind, SessionState};

// Screens below are real captures from agent-manager's status tests
// (internal/status/status_test.go), which these rules are ported from.

fn assert_classified(kind: SessionKind, cases: &[(&str, &str, Option<Signal>)]) {
    let rules = rules_for(kind);
    for (name, screen, expected) in cases {
        assert_eq!(rules.classify(screen), *expected, "{name}");
    }
}

#[test]
fn claude_screens_classify_like_agent_manager() {
    use Signal::*;
    assert_classified(
        SessionKind::Claude,
        &[
            (
                "active turn",
                "✳ Drizzling… (6s · thinking with medium effort)\n❯ ",
                Some(Working),
            ),
            (
                "long turn",
                "✶ Cooking… (2m14s · esc to interrupt)\n❯ ",
                Some(Working),
            ),
            (
                "done at prompt",
                "✻ Cogitated for 13s\n────\n❯ \n────\n  ⏵⏵ bypass permissions on",
                Some(Finished),
            ),
            (
                "done, blank before separator",
                "✻ Cooked for 10s\n\n────\n❯ \n────\n  ▎ ○ Haiku 4.5",
                Some(Finished),
            ),
            (
                "trust dialog",
                " ❯ 1. Yes, I trust this folder\n   2. No, exit\n Enter to confirm · Esc to cancel",
                Some(Waiting),
            ),
            (
                "permission ask",
                "Do you want to proceed?\n ❯ 1. Yes\n   2. No, and tell Claude what to do differently",
                Some(Waiting),
            ),
            (
                "mixed approval",
                "✶ Cooking… (2m 14s · esc to interrupt)\nDo you want to proceed?\n ❯ 1. Yes\n   2. Yes, and don't ask again\n   3. No, and tell Claude what to do differently",
                Some(Waiting),
            ),
            (
                "numbered draft is not a dialog",
                "✳ Drizzling… (6s · esc to interrupt)\n❯ 1. refactor the parser",
                Some(Working),
            ),
            (
                "plain-text question",
                "⏺ What color now, what color want?\n✻ Crunched for 9s\n────\n❯ \n────\n  ▎ ✧ /plan  enter plan mode",
                Some(Waiting),
            ),
            (
                "old question, newer turn",
                "⏺ What color now?\n✻ Crunched for 9s\n  DONE\n✻ Worked for 10s\n────\n❯ \n────",
                Some(Finished),
            ),
            (
                "interrupted turn",
                "  221\n⎿  Interrupted · What should Claude do instead?\n────\n❯ \n────\n  ⏵⏵ bypass permissions on",
                Some(Waiting),
            ),
            (
                "background agents",
                "⏺ Security agent done. 2 left (logic, backend/API).\n✻ Waiting for 2 background agents to finish\n────\n❯ \n────\n  ⏵⏵ bypass permissions on",
                Some(Working),
            ),
            (
                "background agents superseded",
                "✻ Waiting for 2 background agents to finish\n⏺ all agents reported\n✻ Worked for 5s\n────\n❯ \n────",
                Some(Finished),
            ),
            (
                "background shell",
                "⏺ ok\n✻ Worked for 3s · 1 shell still running\n────\n❯ \n────\n  ⏵⏵ bypass permissions on · 1 shell",
                Some(Working),
            ),
            (
                "usage limit",
                "  ⎿  You've hit your weekly limit · resets 1am (Asia/Jerusalem)\n     /usage-credits to finish what you’re working on.\n\n✻ Churned for 2h 0m 54s\n────\n❯ \n────",
                Some(Waiting),
            ),
            (
                "old limit, newer turn",
                "  ⎿  You've hit your weekly limit · resets 1am (Asia/Jerusalem)\n✻ Churned for 2h 0m 54s\n  All done now.\n✻ Worked for 5s\n────\n❯ \n────",
                Some(Finished),
            ),
            (
                "streaming without spinner",
                "  183\n  184\n────\n❯ \n────\n  ▎ ● Fable 5 ✦ medium",
                None,
            ),
            (
                "fresh start with draft",
                "Try \"fix the build\"\n❯ count from 1 to 300",
                None,
            ),
            (
                "question dialog",
                "✻ Churned for 38s\n\n❯ use the AskUserQuestion tool to ask me tabs vs spaces\n\n⏺ Tabs or spaces for indentation?\n────\n ☐ Indent\n\nTabs or spaces for indentation?\n\n❯ 1. Spaces\n     Fixed-width indent. Renders identical everywhere.\n  2. Tabs\n     One tab per level.\n  3. Type something.\n────\n  4. Chat about this\n\nEnter to select · ↑/↓ to navigate · Esc to cancel\n",
                Some(Waiting),
            ),
            (
                "sent numbered message is not a dialog",
                "⏺ Say go on 1 and 2 and I will build the project.\n✻ Brewed for 6m 49s · 9 messages hidden (/focus to show)\n\n❯ 1. I think we should make a space for marketing & sales right? 2. lets\n  create a local git? the other pane showed:\n\n   ❯ 1. Yes\n     2. No\n   Enter to confirm · Esc to cancel\n\n⏺ User message cut off mid-sentence; awaiting clarification\n  ⎿  $ source ~/.profile 2>/dev/null\n\n· Razzle-dazzling… (12m 25s · ↓ 30.1k tokens)\n\n────\n❯ \n────\n  ⏵⏵ auto mode on (shift+tab to cycle)",
                Some(Working),
            ),
        ],
    );
}

#[test]
fn codex_screens_classify_like_agent_manager() {
    use Signal::*;
    assert_classified(
        SessionKind::Codex,
        &[
            (
                "idle at prompt",
                "› Ask Codex to do anything\n  gpt-5.6-terra medium · /home/dev",
                None,
            ),
            (
                "active turn",
                "• Working (0s • esc to interrupt)\n\n› Ask Codex to do anything\n  gpt-5.6-terra medium · /home/dev",
                Some(Working),
            ),
            (
                "animations disabled",
                "Working (12s • esc to interrupt)\n\n› Ask Codex to do anything\n  gpt-5.6-terra medium · /home/dev",
                Some(Working),
            ),
            (
                "reconnecting with details",
                "• Reconnecting... 3/5 (1m 04s • esc to interrupt)\n  └ Stream disconnected before completion\n\n› Ask Codex to do anything\n  gpt-5.6-terra medium · /home/dev",
                Some(Working),
            ),
            (
                "numbered draft is not a dialog",
                "• Working (0s • esc to interrupt)\n\n› 1. keep this as ordinary input\n  gpt-5.6-terra medium · /home/dev",
                Some(Working),
            ),
            (
                "finished turn",
                "• Ran echo preparing\n  └ preparing\n\n────────────────────────────────\n\n• Final response.\n\n─ Worked for 2m 05s ─────────────\n\n› Ask Codex to do anything\n  gpt-5.6-terra medium · /home/dev",
                Some(Finished),
            ),
            (
                "finished on a question",
                "• Which file should I edit, A or B?\n\n─ Worked for 3s ─────────────────\n\n› Ask Codex to do anything\n  gpt-5.6-terra medium · /home/dev",
                Some(Waiting),
            ),
            (
                "recovered after an error",
                "─ Worked for 1s ─────────────\n\n■ unexpected status 404 Not Found: Unknown error\n\n› fix it?\n\n• Fixed. PR #298792 is ready.\n\n────────────────────────────────\n\n› Ask Codex to do anything\n  gpt-5.6-terra medium · /home/dev",
                Some(Finished),
            ),
            (
                "approval modal",
                "  $ echo hello world\n\n› 1. Yes, proceed (y)\n  2. Yes, and don't ask again for commands that start with `echo hello world` (p)\n  3. No, and tell Codex what to do differently (esc)\n\n  Press enter to confirm or esc to cancel",
                Some(Waiting),
            ),
            (
                "approval over stale working",
                "• Working (0s • esc to interrupt)\n\n  $ echo hello world\n\n› 1. Yes, proceed (y)\n  2. No, and tell Codex what to do differently (esc)\n\n  Press enter to confirm or esc to cancel",
                Some(Waiting),
            ),
            (
                "trust dialog",
                "Do you trust the contents of this directory? Working with untrusted contents comes with higher risk of prompt injection.\n\n› 1. Yes, continue\n  2. No, quit\n\n  Press enter to continue",
                Some(Waiting),
            ),
            (
                "user input selection",
                "  Choose an option.\n\n  › 1. Option 1  First choice.\n    2. Option 2  Second choice.\n\n  tab to add notes | enter to submit answer | esc to interrupt",
                Some(Waiting),
            ),
            (
                "usage limit",
                "■ You've hit your usage limit. Upgrade to Plus to continue using Codex, or try again at Jul 22nd, 2026 10:42 AM.\n\n› Ask Codex to do anything",
                Some(Waiting),
            ),
            (
                "sent numbered message is not a dialog",
                "  This deserves a dedicated CV, not the generic version.\n\n─ Worked for 2m 39s ──────────────────────────────────────────\n\n› 1. should I use my regular cv or 2. match it to them?\n  keep the tone hands-on rather than managerial\n\n• Working (12s • esc to interrupt)\n\n› Ask Codex to do anything\n",
                Some(Working),
            ),
        ],
    );
}

#[test]
fn gemini_screens_classify_like_agent_manager() {
    use Signal::*;
    assert_classified(
        SessionKind::Gemini,
        &[
            (
                "idle at prompt",
                " Gemini CLI v0.53.0\nTips for getting started:\n1. Create GEMINI.md files to customize your interactions\n──────────────────────────────\n Shift+Tab to accept edits\n▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄\n >   Type your message or @path/to/file\n▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n workspace (/directory)   branch   sandbox   /model",
                None,
            ),
            (
                "active turn",
                " Press Ctrl+O to show more lines of the last response\n ⠧ Thinking... (esc to cancel, 4s)                        ? for shortcuts\n──────────────────────────────\n Shift+Tab to accept edits\n▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄\n >   Type your message or @path/to/file\n▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀",
                Some(Working),
            ),
            (
                "tool confirmation",
                "╭──────────────────────────────────────╮\n│ Edit example.txt                     │\n│ Apply this change?                   │\n│ ● 1. Allow once                      │\n│   2. Allow always                    │\n│   3. No, suggest changes (esc)       │\n╰──────────────────────────────────────╯\n⡏ Waiting for user confirmation...",
                Some(Waiting),
            ),
            (
                "usage limit",
                "╭──────────────────────────────────────╮\n│ Usage limit reached for gemini-3.5-flash.  │\n│ ● 1. Keep trying                     │\n│   2. Stop                            │\n╰──────────────────────────────────────╯",
                Some(Waiting),
            ),
        ],
    );
}

const CODEX_IDLE: &str = "› Ask Codex to do anything\n  gpt-5.6-terra medium · /home/dev";
const CODEX_WORKING: &str =
    "• Working (0s • esc to interrupt)\n\n› Ask Codex to do anything\n  gpt-5.6-terra medium";
const CODEX_FINISHED: &str = "• Final response.\n\n─ Worked for 2m 05s ─────────────\n\n› Ask Codex to do anything\n  gpt-5.6-terra medium";

#[test]
fn startup_screen_is_a_baseline_not_a_turn() {
    let rules = rules_for(SessionKind::Codex);
    let start = Instant::now();
    let mut tracker = ScreenTracker::new(start);
    assert_eq!(
        tracker.derive(rules, "loading", SessionState::Idle, None, start),
        None
    );
    assert_eq!(
        tracker.derive(rules, CODEX_IDLE, SessionState::Idle, None, start),
        None
    );
    assert_eq!(
        tracker.derive(rules, CODEX_IDLE, SessionState::Idle, None, start),
        Some(SessionState::Idle)
    );
}

#[test]
fn a_turn_runs_busy_then_rests_ready_until_viewed() {
    let rules = rules_for(SessionKind::Codex);
    let start = Instant::now();
    let mut tracker = ScreenTracker::new(start);
    tracker.derive(rules, CODEX_IDLE, SessionState::Idle, None, start);

    let busy = tracker.derive(rules, CODEX_WORKING, SessionState::Idle, None, start);
    assert_eq!(busy, Some(SessionState::Busy));
    let ready = tracker.derive(rules, CODEX_FINISHED, SessionState::Busy, None, start);
    assert_eq!(ready, Some(SessionState::Ready));
    // Once viewed, the same finished screen keeps the session idle.
    let viewed = tracker.derive(rules, CODEX_FINISHED, SessionState::Idle, None, start);
    assert_eq!(viewed, Some(SessionState::Idle));
}

#[test]
fn quiet_screen_ends_a_turn_after_the_grace_period() {
    let rules = rules_for(SessionKind::Custom);
    let start = Instant::now();
    let mut tracker = ScreenTracker::new(start);
    tracker.derive(rules, "$ run\n", SessionState::Idle, None, start);

    let changed = tracker.derive(rules, "$ run\nstep 1\n", SessionState::Idle, None, start);
    assert_eq!(changed, Some(SessionState::Busy));
    let pause = start + Duration::from_millis(500);
    let quiet = tracker.derive(rules, "$ run\nstep 1\n", SessionState::Busy, None, pause);
    assert_eq!(quiet, Some(SessionState::Busy));
    let later = pause + Duration::from_millis(1_100);
    let ended = tracker.derive(rules, "$ run\nstep 1\n", SessionState::Busy, None, later);
    assert_eq!(ended, Some(SessionState::Ready));
}

#[test]
fn quiet_screen_ending_on_a_question_waits() {
    let rules = rules_for(SessionKind::Custom);
    let start = Instant::now();
    let mut tracker = ScreenTracker::new(start);
    tracker.derive(rules, "$ run\n", SessionState::Idle, None, start);
    tracker.derive(rules, "Continue?\n", SessionState::Idle, None, start);
    tracker.derive(rules, "Continue?\n", SessionState::Busy, None, start);

    let later = start + Duration::from_secs(2);
    let ended = tracker.derive(rules, "Continue?\n", SessionState::Busy, None, later);
    assert_eq!(ended, Some(SessionState::Waiting));
}

#[test]
fn hooks_win_except_where_the_screen_sees_more() {
    let rules = rules_for(SessionKind::Claude);
    let dialog =
        "Do you want to proceed?\n ❯ 1. Yes\n   2. No, and tell Claude what to do differently";
    let finished = "✻ Cogitated for 13s\n────\n❯ \n────";
    let streaming = "  183\n  184\n────\n❯ \n────";

    assert_eq!(apply_hook(rules, dialog, Signal::Finished), Signal::Waiting);
    // Esc fires no Stop, so a turn the screen shows as ended is over.
    assert_eq!(
        apply_hook(rules, finished, Signal::Working),
        Signal::Finished
    );
    assert_eq!(
        apply_hook(rules, streaming, Signal::Working),
        Signal::Working
    );
    assert_eq!(
        apply_hook(rules, streaming, Signal::Finished),
        Signal::Finished
    );

    let mut tracker = ScreenTracker::new(Instant::now());
    let state = tracker.derive(
        rules,
        streaming,
        SessionState::Idle,
        Some(Signal::Idle),
        Instant::now(),
    );
    assert_eq!(state, Some(SessionState::Idle));
}

#[test]
fn unviewed_ready_survives_an_idle_reading() {
    assert_eq!(
        resolve(SessionState::Ready, Signal::Idle),
        SessionState::Ready
    );
    assert_eq!(
        resolve(SessionState::Busy, Signal::Idle),
        SessionState::Idle
    );
    assert_eq!(
        resolve(SessionState::Idle, Signal::Finished),
        SessionState::Idle
    );
    assert_eq!(
        resolve(SessionState::Waiting, Signal::Finished),
        SessionState::Ready
    );
}
