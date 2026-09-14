pub fn cleanup_copied_text(text: &str) -> String {
    text.replace('\u{00a0}', " ")
        .lines()
        .map(|line| line.strip_prefix("  ").unwrap_or(line).trim_end())
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_owned()
}
