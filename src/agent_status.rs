//! Derives an agent session's state from its visible terminal screen, combined with
//! explicit hook signals when the agent reports them.
//!
//! Ported from agent-manager's status engine (internal/status/status.go) and poller
//! (internal/ui/poller.go): https://github.com/YoanWai/agent-manager

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use regex::Regex;

use crate::control::{SessionKind, SessionState};

/// What a hook or the screen says about the current turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Working,
    Waiting,
    Finished,
    Idle,
}

/// How long a working region must stay unchanged before the turn counts as ended.
/// Agents pause between tools, so one quiet poll is not enough.
const QUIET_END_GRACE: Duration = Duration::from_secs(1);
/// A spinner row that sits unchanged this long belongs to a turn that died.
const STUCK_END_GRACE: Duration = Duration::from_secs(15);
/// Caps how long a session may stay in its launch phase.
const STARTING_GRACE: Duration = Duration::from_secs(30);

struct Rule {
    signal: Signal,
    re: Regex,
}

pub struct ToolRules {
    /// The tool's input box; everything above its last match is the activity region.
    activity_cutoff: Option<Regex>,
    /// Without a cutoff, treat the whole screen as the activity region.
    whole_screen_region: bool,
    turn_end: Option<Regex>,
    chrome_line: Option<Regex>,
    blocked_line: Option<Regex>,
    trailing_note: Option<Regex>,
    busy_line: Option<Regex>,
    limit_line: Option<Regex>,
    dialog_footer: Option<Regex>,
    rules: Vec<Rule>,
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("built-in status pattern is valid")
}

fn rule(signal: Signal, pattern: &str) -> Rule {
    Rule {
        signal,
        re: re(pattern),
    }
}

static CLAUDE: LazyLock<ToolRules> = LazyLock::new(|| ToolRules {
    activity_cutoff: Some(re(r"(?m)^❯")),
    whole_screen_region: false,
    turn_end: Some(re(r"^[✻✳✶✽✢·✦✧+*] \S+ for \d.*$")),
    chrome_line: Some(re(
        r"^\s*[─q]{4,}.*$|^[\s─q]*$|^\s*✔ Update installed · Restart to update\s*$|^\s*new task\? /clear to save .*$",
    )),
    blocked_line: Some(re("Interrupted ·")),
    trailing_note: Some(re("^※")),
    busy_line: Some(re(
        r"^[✻✳✶✽✢·✦✧+*] (?:Waiting for \d+ background agents? to finish|.*· \d+ shells? still running)",
    )),
    limit_line: Some(re(r"(?m)You've hit your .+limit")),
    dialog_footer: Some(re(r"(?m)^\s*Enter to select\b")),
    rules: vec![
        rule(Signal::Waiting, "Enter to confirm"),
        rule(Signal::Waiting, r"(?m)^[ \x{A0}]*❯[ \x{A0}]+\d+\."),
        rule(Signal::Working, r"(?m)^[✻✳✶✽✢·✦✧+*] \S+… \("),
        rule(Signal::Working, "esc to interrupt"),
    ],
});

static CODEX: LazyLock<ToolRules> = LazyLock::new(|| ToolRules {
    activity_cutoff: Some(re(r"(?m)^›")),
    whole_screen_region: false,
    turn_end: Some(re(r"(?m)^(?:─+ Worked for [\dhms. ]+─+|─+)$")),
    chrome_line: Some(re(r"^\s*─*\s*$")),
    blocked_line: None,
    trailing_note: None,
    busy_line: None,
    limit_line: Some(re(r"(?m)You've hit your usage limit")),
    dialog_footer: None,
    rules: vec![
        rule(Signal::Waiting, r"(?m)^\s*›\s+\d+\."),
        rule(Signal::Waiting, r"(?m)Press enter to (confirm|continue)\b"),
        rule(Signal::Waiting, r"(?m)enter to submit answer\b"),
        // The status row sits right above the input box; anchoring its full shape
        // keeps an answer that quotes "esc to interrupt" from looking active.
        rule(
            Signal::Working,
            r"(?m)^[ \t]*(?:• )?[^\n]*\([\dhms. ]+ [•·] esc to interrupt\)(?: · [^\n]*)?[ \t]*\n(?:[ \t]+└[^\n]*\n(?:[ \t]{4}[^\n]*\n)*)?[ \t\n]*\z",
        ),
    ],
});

static GEMINI: LazyLock<ToolRules> = LazyLock::new(|| ToolRules {
    // The composer: "> " normally, "! " in shell mode, "* " in yolo mode.
    activity_cutoff: Some(re(r"(?m)^\s*[>!*] ")),
    whole_screen_region: false,
    turn_end: None,
    // The "? for shortcuts" hint must not read as a question.
    chrome_line: Some(re(
        r"^\s*[╭╮╰╯│─▄▀█]*\s*$|^\s*\? for shortcuts\s*$|^\s*press tab twice for more\s*$|^\s*Press Ctrl\+O to show more lines.*$|(?i)^\s*(auto-accept edits |plan |yolo )?\S*tab\S* to (accept edits|manual|plan|auto-accept edits)\b.*$",
    )),
    blocked_line: None,
    trailing_note: None,
    busy_line: None,
    limit_line: Some(re("Usage limit reached")),
    dialog_footer: None,
    rules: vec![
        rule(Signal::Waiting, r"(?m)^[\s│]*●\s*\d+\."),
        rule(Signal::Waiting, "Waiting for user confirmation"),
        rule(Signal::Working, "esc to cancel"),
    ],
});

/// Tools without rules still get quiet detection over the whole screen.
static GENERIC: LazyLock<ToolRules> = LazyLock::new(|| ToolRules {
    activity_cutoff: None,
    whole_screen_region: true,
    turn_end: None,
    chrome_line: None,
    blocked_line: None,
    trailing_note: None,
    busy_line: None,
    limit_line: None,
    dialog_footer: None,
    rules: Vec::new(),
});

pub fn rules_for(kind: SessionKind) -> &'static ToolRules {
    match kind {
        SessionKind::Claude => &CLAUDE,
        SessionKind::Codex => &CODEX,
        SessionKind::Gemini => &GEMINI,
        SessionKind::Shell | SessionKind::Custom => &GENERIC,
    }
}

fn trim_row(line: &str) -> &str {
    line.trim_end_matches([' ', '\t'])
}

impl ToolRules {
    /// The screen's status, or None when no signal matched.
    pub fn classify(&self, screen: &str) -> Option<Signal> {
        // A usage limit blocks the turn on the user.
        if self.is_limit(screen) {
            return Some(Signal::Waiting);
        }
        if let Some(signal) = self.match_rules(&self.match_scope(screen)) {
            return Some(signal);
        }
        if self.is_busy(screen) {
            return Some(Signal::Working);
        }
        self.turn_state(screen)
    }

    /// The screen above the tool's input box.
    pub fn activity_region<'a>(&self, screen: &'a str) -> Option<&'a str> {
        let Some(cutoff) = &self.activity_cutoff else {
            return self.whole_screen_region.then_some(screen);
        };
        cutoff
            .find_iter(screen)
            .last()
            .map(|found| &screen[..found.start()])
    }

    /// Resting state of a turn that stopped changing without a turn-end marker.
    /// A question on the last content line waits on the user.
    pub fn turn_ended_state(&self, region: &str) -> Signal {
        let lines = region.split('\n').collect::<Vec<_>>();
        match last_content_index(&lines, lines.len() as isize - 1, self.chrome_line.as_ref()) {
            Some(last) if lines[last].contains('?') => Signal::Waiting,
            _ => Signal::Finished,
        }
    }

    fn match_rules(&self, scope: &str) -> Option<Signal> {
        for (index, rule) in self.rules.iter().enumerate() {
            if !rule.re.is_match(scope) {
                continue;
            }
            // A later waiting rule wins over working, so rule order cannot mask a prompt.
            if rule.signal == Signal::Working
                && self.rules[index + 1..]
                    .iter()
                    .any(|later| later.signal == Signal::Waiting && later.re.is_match(scope))
            {
                return Some(Signal::Waiting);
            }
            return Some(rule.signal);
        }
        None
    }

    fn is_limit(&self, screen: &str) -> bool {
        let Some(limit) = &self.limit_line else {
            return false;
        };
        let Some(region) = self.activity_region(screen) else {
            return limit.is_match(screen);
        };
        let lines = region.split('\n').collect::<Vec<_>>();
        for index in (0..lines.len()).rev() {
            if !limit.is_match(trim_row(lines[index])) {
                continue;
            }
            let mut end = index + 1;
            while end < lines.len() {
                let line = lines[end];
                if line.trim().is_empty() || !(line.starts_with(' ') || line.starts_with('\t')) {
                    break;
                }
                end += 1;
            }
            return self.settled_below(&lines[end..], true);
        }
        false
    }

    /// Background agents and shells outlive the turn that started them.
    fn is_busy(&self, screen: &str) -> bool {
        let Some(busy) = &self.busy_line else {
            return false;
        };
        let Some(region) = self.activity_region(screen) else {
            return false;
        };
        let lines = region.split('\n').collect::<Vec<_>>();
        for index in (0..lines.len()).rev() {
            if busy.is_match(trim_row(lines[index])) {
                return self
                    .last_turn_end_index(&lines)
                    .is_none_or(|end| end <= index);
            }
        }
        false
    }

    /// Limits rule matching to the current turn, so older turns that quote
    /// spinner or dialog text cannot read as live signals.
    fn match_scope(&self, screen: &str) -> String {
        if self.turn_end.is_none() {
            return screen.to_owned();
        }
        let Some(region) = self.activity_region(screen) else {
            return screen.to_owned();
        };
        let cutoff_tail = &screen[region.len()..];
        let has_waiting_footer = self.has_waiting_footer(cutoff_tail);
        let lines = region.split('\n').collect::<Vec<_>>();
        if let Some(last_end) = self.last_turn_end_index(&lines) {
            let scope = self.without_input_rows(&lines[last_end + 1..]);
            return if has_waiting_footer {
                scope + cutoff_tail
            } else {
                scope
            };
        }
        if has_waiting_footer {
            return screen.to_owned();
        }
        self.without_input_rows(&lines)
    }

    /// Drops prompts the user already sent, which the tool replays behind its
    /// input marker and could otherwise look like a dialog option.
    fn without_input_rows(&self, lines: &[&str]) -> String {
        let mut kept = Vec::with_capacity(lines.len());
        let mut sent = false;
        for line in lines {
            if self.input_row(line) {
                sent = true;
                continue;
            }
            if sent && wraps_above(line) {
                continue;
            }
            sent = false;
            kept.push(*line);
        }
        kept.join("\n")
    }

    fn has_waiting_footer(&self, cutoff_tail: &str) -> bool {
        let Some(line_end) = cutoff_tail.find('\n') else {
            return false;
        };
        let footer = &cutoff_tail[line_end + 1..];
        if self
            .dialog_footer
            .as_ref()
            .is_some_and(|dialog| dialog.is_match(footer))
        {
            return true;
        }
        self.rules
            .iter()
            .any(|rule| rule.signal == Signal::Waiting && rule.re.is_match(footer))
    }

    fn last_turn_end_index(&self, lines: &[&str]) -> Option<usize> {
        let turn_end = self.turn_end.as_ref()?;
        (0..lines.len())
            .rev()
            .find(|&index| turn_end.is_match(trim_row(lines[index])))
    }

    fn input_row(&self, row: &str) -> bool {
        self.activity_cutoff
            .as_ref()
            .and_then(|cutoff| cutoff.find(row))
            .is_some_and(|found| found.start() == 0 && !found.is_empty())
    }

    /// Reads the newest turn: finished when only chrome sits below the last
    /// turn-end marker, waiting when the agent ended on a question.
    fn turn_state(&self, screen: &str) -> Option<Signal> {
        self.turn_end.as_ref()?;
        let region = self.activity_region(screen)?;
        let lines = region.split('\n').collect::<Vec<_>>();
        let last = last_content_index(&lines, lines.len() as isize - 1, self.chrome_line.as_ref())?;
        if self
            .blocked_line
            .as_ref()
            .is_some_and(|blocked| blocked.is_match(lines[last]))
        {
            return Some(Signal::Waiting);
        }
        let last_end = self.last_turn_end_index(&lines)?;
        if !self.settled_below(&lines[last_end + 1..], false) {
            return None;
        }
        match last_content_index(&lines, last_end as isize - 1, None) {
            Some(question) if lines[question].contains('?') => Some(Signal::Waiting),
            _ => Some(Signal::Finished),
        }
    }

    /// Whether only blanks, chrome and trailing notes follow a marker.
    fn settled_below(&self, after: &[&str], mut skip_turn_end: bool) -> bool {
        let mut in_note = false;
        for line in after {
            let trimmed = trim_row(line);
            if trimmed.trim().is_empty() {
                continue;
            }
            if self
                .chrome_line
                .as_ref()
                .is_some_and(|chrome| chrome.is_match(trimmed))
            {
                continue;
            }
            if skip_turn_end
                && self
                    .turn_end
                    .as_ref()
                    .is_some_and(|turn_end| turn_end.is_match(trimmed))
            {
                skip_turn_end = false;
                continue;
            }
            if self
                .trailing_note
                .as_ref()
                .is_some_and(|note| note.is_match(trimmed.trim_start_matches([' ', '\t'])))
            {
                in_note = true;
                continue;
            }
            if in_note {
                continue;
            }
            return false;
        }
        true
    }
}

/// Tools indent what wraps and leave blank rows between blocks empty.
fn wraps_above(row: &str) -> bool {
    let body = row.trim_start();
    body.is_empty() || body.len() < row.len()
}

fn last_content_index(lines: &[&str], start: isize, chrome: Option<&Regex>) -> Option<usize> {
    (0..=start)
        .rev()
        .map(|index| index as usize)
        .find(|&index| {
            let line = lines[index];
            !line.trim().is_empty() && !chrome.is_some_and(|chrome| chrome.is_match(trim_row(line)))
        })
}

fn hash_text(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// A turn is running or resting unacknowledged; only then can a quiet screen end it.
fn in_flight(state: SessionState) -> bool {
    matches!(
        state,
        SessionState::Busy | SessionState::Ready | SessionState::Waiting
    )
}

/// Maps a signal onto session state. A finished turn the user already viewed
/// stays idle, and an unviewed one stays ready until it is viewed.
pub fn resolve(current: SessionState, signal: Signal) -> SessionState {
    match signal {
        Signal::Working => SessionState::Busy,
        Signal::Waiting => SessionState::Waiting,
        Signal::Finished if current == SessionState::Idle => SessionState::Idle,
        Signal::Finished => SessionState::Ready,
        Signal::Idle if current == SessionState::Ready => SessionState::Ready,
        Signal::Idle => SessionState::Idle,
    }
}

/// Hooks cannot see everything, so the screen may override them: a dialog or
/// error after Stop, or a turn that ended without Stop (Esc, background agents).
pub fn apply_hook(rules: &ToolRules, screen: &str, hook: Signal) -> Signal {
    let screen_signal = rules.classify(screen);
    match (hook, screen_signal) {
        (Signal::Finished, Some(signal @ (Signal::Waiting | Signal::Working))) => signal,
        (Signal::Working, Some(signal @ (Signal::Waiting | Signal::Finished))) => signal,
        _ => hook,
    }
}

#[derive(Debug, Clone, Copy)]
struct QuietTimer {
    since: Instant,
    stuck: bool,
    screen_hash: u64,
}

/// Per-session memory between polls.
#[derive(Debug, Clone)]
pub struct ScreenTracker {
    started: Instant,
    booted: bool,
    region_hash: Option<u64>,
    quiet: Option<QuietTimer>,
}

impl ScreenTracker {
    pub fn new(now: Instant) -> Self {
        Self {
            started: now,
            booted: false,
            region_hash: None,
            quiet: None,
        }
    }

    /// The session's next state, or None while the agent is still starting.
    pub fn derive(
        &mut self,
        rules: &ToolRules,
        screen: &str,
        current: SessionState,
        hook: Option<Signal>,
        now: Instant,
    ) -> Option<SessionState> {
        if let Some(hook) = hook {
            self.booted = true;
            return Some(resolve(current, apply_hook(rules, screen, hook)));
        }
        let region = rules.activity_region(screen);
        let region_hash = region.map(hash_text);
        if !self.booted {
            // Startup painting is not a turn. Wait for the input box, or any output
            // for tools without one, and take that screen as the baseline.
            let painted = region
                .is_some_and(|region| rules.activity_cutoff.is_some() || !region.trim().is_empty());
            if painted || now.duration_since(self.started) >= STARTING_GRACE {
                self.booted = true;
                self.region_hash = region_hash;
            }
            return None;
        }

        let classified = rules.classify(screen);
        let matched = classified.is_some();
        let mut signal = classified.unwrap_or(Signal::Idle);
        let stuck = classified == Some(Signal::Working);
        let previous = self.region_hash;
        self.region_hash = region_hash;
        match (region, region_hash) {
            (Some(region), Some(hash)) if !matched || stuck => match previous {
                Some(previous) if previous != hash => {
                    if !matched {
                        signal = Signal::Working;
                    }
                    self.quiet = None;
                }
                Some(_) if in_flight(current) => {
                    if current != SessionState::Busy {
                        // Already resting: re-read finished versus waiting without delay.
                        if !stuck {
                            signal = rules.turn_ended_state(region);
                        }
                    } else {
                        let (grace, screen_hash) = if stuck {
                            (STUCK_END_GRACE, hash_text(screen))
                        } else {
                            (QUIET_END_GRACE, 0)
                        };
                        let timer = match self.quiet {
                            Some(timer)
                                if timer.stuck == stuck && timer.screen_hash == screen_hash =>
                            {
                                timer
                            }
                            _ => QuietTimer {
                                since: now,
                                stuck,
                                screen_hash,
                            },
                        };
                        self.quiet = Some(timer);
                        if now.duration_since(timer.since) >= grace {
                            signal = rules.turn_ended_state(region);
                            if signal != Signal::Working {
                                self.quiet = None;
                            }
                        } else {
                            signal = Signal::Working;
                        }
                    }
                }
                Some(_) => {}
                None if in_flight(current) && !stuck => return Some(current),
                None => {}
            },
            _ => self.quiet = None,
        }
        Some(resolve(current, signal))
    }
}
