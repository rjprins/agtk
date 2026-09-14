pub fn cleanup_copied_text(text: &str) -> String {
    text.replace('\u{00a0}', " ")
        .lines()
        .map(|line| line.strip_prefix("  ").unwrap_or(line).trim_end())
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_owned()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedTerminalText {
    pub text: String,
    pub lines: usize,
    pub is_truncated: bool,
}

pub fn bounded_terminal_text(text: &str, max_lines: usize) -> BoundedTerminalText {
    let normalized = text.replace('\u{00a0}', " ");
    let mut lines = normalized.lines().map(str::trim_end).collect::<Vec<_>>();
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }

    let is_truncated = lines.len() > max_lines;
    let first_line = lines.len().saturating_sub(max_lines);
    let text = lines[first_line..].join("\n");
    BoundedTerminalText {
        lines: lines.len() - first_line,
        text,
        is_truncated,
    }
}
