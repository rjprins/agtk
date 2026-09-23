//! File paths in terminal output that Ctrl+click opens in Emacs.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

const MAX_PATH_LENGTH: usize = 1024;

/// VTE (PCRE2) pattern for path-like text: anything with a slash, or a bare file name
/// with an extension, plus an optional `:line[:col]`, `(line[,col])` or Python
/// `", line N` suffix. The lookbehind keeps matches from starting inside a word or URL.
pub const TERMINAL_PATTERN: &str = r#"(?<![\w./~@+:-])(?:(?:~|\.{1,2})?(?:/[\w.@+-]+)+|[\w@+-][\w.@+-]*(?:/[\w.@+-]+)+|[\w@+-]+(?:\.[\w@+-]+)*\.[A-Za-z][\w-]{0,9}(?![\w@+-]|\.\w))(?::\d+(?::\d+)?|\(\d+(?:, ?\d+)?\)|", line \d+)?"#;

static LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"^(?P<path>.+?)(?::(?P<line>\d+)(?::(?P<column>\d+))?|\((?P<pline>\d+)(?:, ?(?P<pcolumn>\d+))?\)|", line (?P<pyline>\d+))?$"#,
    )
    .expect("file link pattern is valid")
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileLink {
    pub path: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

/// Splits clicked terminal text into a path and an optional position.
pub fn parse_link(text: &str) -> Option<FileLink> {
    let captures = LINK.captures(text.trim())?;
    // A sentence-ending dot is punctuation, not part of the file name.
    let path = captures["path"].trim_end_matches('.');
    if path.is_empty() || path.len() > MAX_PATH_LENGTH || path.chars().all(|c| c == '/') {
        return None;
    }
    let number = |names: [&str; 2]| {
        names
            .iter()
            .find_map(|name| captures.name(name))
            .and_then(|value| value.as_str().parse::<u32>().ok())
            .filter(|value| *value > 0)
    };
    Some(FileLink {
        path: path.to_owned(),
        line: number(["line", "pline"]).or_else(|| number(["pyline", "pyline"])),
        column: number(["column", "pcolumn"]),
    })
}

/// Resolves a path seen in terminal output to an existing regular file.
/// Relative paths are tried against each base directory in order.
pub fn resolve(raw: &str, bases: &[PathBuf], home: &Path) -> Option<PathBuf> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > MAX_PATH_LENGTH || raw.contains('\0') {
        return None;
    }
    let expanded = if raw == "~" {
        home.to_path_buf()
    } else if let Some(rest) = raw.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(raw)
    };
    if expanded.is_absolute() {
        return expanded.is_file().then_some(expanded);
    }
    // `git diff` prints a/<path> and b/<path>.
    let mut variants = vec![expanded.clone()];
    if let Some(stripped) = raw.strip_prefix("a/").or_else(|| raw.strip_prefix("b/")) {
        variants.push(PathBuf::from(stripped));
    }
    variants.iter().find_map(|variant| {
        bases
            .iter()
            .map(|base| base.join(variant))
            .find(|candidate| candidate.is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::{FileLink, parse_link, resolve};
    use std::path::PathBuf;

    fn link(path: &str, line: Option<u32>, column: Option<u32>) -> Option<FileLink> {
        Some(FileLink {
            path: path.to_owned(),
            line,
            column,
        })
    }

    #[test]
    fn parses_positions_in_the_common_formats() {
        assert_eq!(
            parse_link("src/ui/mod.rs"),
            link("src/ui/mod.rs", None, None)
        );
        assert_eq!(
            parse_link("src/main.rs:42"),
            link("src/main.rs", Some(42), None)
        );
        assert_eq!(
            parse_link("src/main.rs:42:7"),
            link("src/main.rs", Some(42), Some(7))
        );
        assert_eq!(
            parse_link("Program.cs(12,5)"),
            link("Program.cs", Some(12), Some(5))
        );
        assert_eq!(
            parse_link("/app/x.py\", line 9"),
            link("/app/x.py", Some(9), None)
        );
        assert_eq!(parse_link("README.md."), link("README.md", None, None));
        assert_eq!(parse_link("//"), None);
    }

    #[test]
    fn resolves_relative_diff_and_home_paths() {
        let fixture = tempfile::tempdir().unwrap();
        let cwd = fixture.path().join("repo/sub");
        let root = fixture.path().join("repo");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::write(root.join("top.rs"), "").unwrap();
        std::fs::write(cwd.join("local.rs"), "").unwrap();
        let bases = [cwd.clone(), root.clone()];
        let home = fixture.path();

        assert_eq!(
            resolve("local.rs", &bases, home),
            Some(cwd.join("local.rs"))
        );
        assert_eq!(resolve("top.rs", &bases, home), Some(root.join("top.rs")));
        assert_eq!(resolve("b/top.rs", &bases, home), Some(root.join("top.rs")));
        assert_eq!(
            resolve("~/repo/top.rs", &bases, home),
            Some(root.join("top.rs"))
        );
        assert_eq!(resolve("missing.rs", &bases, home), None);
        // Directories are not files to open.
        assert_eq!(resolve("sub", &[root.clone()], home), None);
        assert_eq!(
            resolve(&root.to_string_lossy(), &bases, home),
            None::<PathBuf>
        );
    }
}
