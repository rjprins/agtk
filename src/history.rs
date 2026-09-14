const MAX_INPUT_CHARS: usize = 512;

pub fn history_needle(text: &str, max_chars: usize) -> String {
    let first_line = text.lines().next().unwrap_or_default();
    let mut compact = first_line.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.ends_with("...") {
        compact.truncate(compact.len() - 3);
        compact = compact.trim_end().to_owned();
    }

    if compact.chars().count() > max_chars {
        let cut = compact.chars().take(max_chars).collect::<String>();
        if let Some(last_space) = cut.rfind(' ')
            && cut[..last_space].chars().count() > max_chars / 2
        {
            compact = cut[..last_space].to_owned();
        } else {
            compact = cut;
        }
    }

    compact.trim().to_owned()
}

#[derive(Debug, Default)]
pub struct InputTracker {
    line: String,
    bracketed_paste: bool,
}

impl InputTracker {
    pub fn push(&mut self, data: &str) -> Option<String> {
        let mut submitted = None;
        let mut offset = 0;

        while offset < data.len() {
            let remaining = &data[offset..];
            if remaining.starts_with("\u{1b}[200~") {
                self.bracketed_paste = true;
                offset += 6;
                continue;
            }
            if remaining.starts_with("\u{1b}[201~") {
                self.bracketed_paste = false;
                offset += 6;
                continue;
            }
            if remaining.starts_with("\u{1b}[") {
                offset += escape_sequence_len(remaining);
                continue;
            }
            if remaining.starts_with('\u{1b}') {
                offset += remaining.chars().take(2).map(char::len_utf8).sum::<usize>();
                continue;
            }

            let character = remaining
                .chars()
                .next()
                .expect("remaining input is non-empty");
            offset += character.len_utf8();
            match character {
                '\r' | '\n' if self.bracketed_paste => self.line.push(' '),
                '\r' | '\n' => {
                    let normalized = self.line.split_whitespace().collect::<Vec<_>>().join(" ");
                    self.line.clear();
                    if !normalized.is_empty() {
                        submitted = Some(normalized);
                    }
                }
                '\u{7f}' | '\u{8}' => {
                    self.line.pop();
                }
                '\u{15}' => self.line.clear(),
                control if control < ' ' => {}
                printable => self.line.push(printable),
            }
        }

        let count = self.line.chars().count();
        if count > MAX_INPUT_CHARS {
            self.line = self.line.chars().skip(count - MAX_INPUT_CHARS).collect();
        }
        submitted
    }
}

fn escape_sequence_len(input: &str) -> usize {
    input
        .as_bytes()
        .iter()
        .skip(2)
        .position(|byte| (0x40..=0x7e).contains(byte))
        .map_or(input.len(), |index| index + 3)
}
