//! File paths in terminal output that Ctrl+click opens in Emacs, and links in rendered Markdown.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
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
    bases.iter().find_map(|base| {
        variants
            .iter()
            .map(|variant| base.join(variant))
            .find(|candidate| candidate.is_file())
    })
}

/// Where a link in a rendered Markdown document leads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkdownLink {
    Web(String),
    File { path: PathBuf, line: Option<u32> },
}

pub fn is_markdown_path(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| {
            ["md", "markdown", "mdown", "mkd"]
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
        })
}

/// Resolves a link in the Markdown file `document`. Relative paths start in the
/// document's directory and `/` paths at the worktree root, as on GitHub. A `#L12`
/// fragment names a line. Other schemes than http, https and mailto are refused,
/// because a custom scheme handler would start another program.
pub fn resolve_markdown_link(
    href: &str,
    document: &Path,
    worktree_root: &Path,
) -> Option<MarkdownLink> {
    let href = href.trim();
    if href.is_empty() || href.len() > MAX_PATH_LENGTH * 4 || href.contains('\0') {
        return None;
    }
    if let Some(scheme) = uri_scheme(href) {
        return ["http", "https", "mailto"]
            .iter()
            .any(|allowed| scheme.eq_ignore_ascii_case(allowed))
            .then(|| MarkdownLink::Web(href.to_owned()));
    }
    if href.starts_with("//") {
        return None;
    }
    let (rest, fragment) = href.split_once('#').unwrap_or((href, ""));
    let (raw_path, _query) = rest.split_once('?').unwrap_or((rest, ""));
    if raw_path.is_empty() {
        return None;
    }
    let decoded = crate::text::percent_decode(raw_path).filter(|bytes| !bytes.contains(&0))?;
    let relative = Path::new(OsStr::from_bytes(&decoded));
    let joined = match relative.strip_prefix("/") {
        Ok(inside) => worktree_root.join(inside),
        Err(_) => document.parent()?.join(relative),
    };
    let line = fragment
        .strip_prefix('L')
        .map(|lines| lines.split('-').next().unwrap_or_default())
        .and_then(|line| line.parse::<u32>().ok())
        .filter(|line| *line > 0);
    Some(MarkdownLink::File {
        path: normalize(&joined),
        line,
    })
}

fn uri_scheme(href: &str) -> Option<&str> {
    let (scheme, _) = href.split_once(':')?;
    let mut characters = scheme.chars();
    let valid = characters.next()?.is_ascii_alphabetic()
        && characters.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    valid.then_some(scheme)
}

/// Removes `.` and `..` without touching the disk, so the same file gets the same tab.
fn normalize(path: &Path) -> PathBuf {
    let mut normal = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normal.pop();
            }
            other => normal.push(other),
        }
    }
    normal
}

#[cfg(test)]
mod tests {
    use super::{
        FileLink, MarkdownLink, is_markdown_path, parse_link, resolve, resolve_markdown_link,
    };
    use std::path::{Path, PathBuf};

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
        assert_eq!(resolve("sub", std::slice::from_ref(&root), home), None);
        assert_eq!(
            resolve(&root.to_string_lossy(), &bases, home),
            None::<PathBuf>
        );
    }

    #[test]
    fn diff_prefixes_respect_base_directory_priority() {
        let fixture = tempfile::tempdir().unwrap();
        let worktree = fixture.path().join("worktree");
        let project = fixture.path().join("project");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(worktree.join("main.rs"), "").unwrap();
        for prefix in ["a", "b"] {
            std::fs::create_dir_all(project.join(prefix)).unwrap();
            std::fs::write(project.join(prefix).join("main.rs"), "").unwrap();
            assert_eq!(
                resolve(
                    &format!("{prefix}/main.rs"),
                    &[worktree.clone(), project.clone()],
                    fixture.path(),
                ),
                Some(worktree.join("main.rs")),
            );
        }
    }

    #[test]
    fn resolves_markdown_links_like_github() {
        let document = Path::new("/repo/docs/guide.md");
        let root = Path::new("/repo");
        let file = |path: &str, line| {
            Some(MarkdownLink::File {
                path: PathBuf::from(path),
                line,
            })
        };

        assert_eq!(
            resolve_markdown_link("setup.md", document, root),
            file("/repo/docs/setup.md", None)
        );
        assert_eq!(
            resolve_markdown_link("./img/../setup.md#install", document, root),
            file("/repo/docs/setup.md", None)
        );
        assert_eq!(
            resolve_markdown_link("../src/main.rs#L12-L20", document, root),
            file("/repo/src/main.rs", Some(12))
        );
        assert_eq!(
            resolve_markdown_link("/README.md?plain=1", document, root),
            file("/repo/README.md", None)
        );
        assert_eq!(
            resolve_markdown_link("My%20Notes.md", document, root),
            file("/repo/docs/My Notes.md", None)
        );
        assert_eq!(
            resolve_markdown_link("https://example.com/a", document, root),
            Some(MarkdownLink::Web("https://example.com/a".to_owned()))
        );
        assert_eq!(
            resolve_markdown_link("javascript:alert(1)", document, root),
            None
        );
        assert_eq!(
            resolve_markdown_link("file:///etc/passwd", document, root),
            None
        );
        assert_eq!(
            resolve_markdown_link("//evil.example/x", document, root),
            None
        );
        assert_eq!(resolve_markdown_link("#section", document, root), None);
        assert_eq!(resolve_markdown_link("bad%zz.md", document, root), None);
        assert_eq!(resolve_markdown_link("bad%+1.md", document, root), None);
        assert_eq!(resolve_markdown_link("nul%00.md", document, root), None);
    }

    #[test]
    fn recognizes_markdown_extensions() {
        assert!(is_markdown_path(Path::new("README.md")));
        assert!(is_markdown_path(Path::new("notes.MARKDOWN")));
        assert!(!is_markdown_path(Path::new("main.rs")));
        assert!(!is_markdown_path(Path::new("md")));
    }
}
