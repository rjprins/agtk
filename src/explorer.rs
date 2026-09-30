//! The worktree file tree shown in the Files sidebar page.

use std::collections::{BTreeMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::changes::{ChangeStatus, ChangesSnapshot};

/// Matches shown for one filter query before the list is cut off.
pub const MAX_FILTER_MATCHES: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileNode {
    /// The last path component, shown as the row label.
    pub name: String,
    /// The worktree-relative path exactly as Git printed it.
    pub path: Vec<u8>,
    pub is_dir: bool,
    /// Directories first, then files, both in case-insensitive order.
    pub children: Vec<Arc<FileNode>>,
}

impl FileNode {
    pub fn display_path(&self) -> String {
        String::from_utf8_lossy(&self.path).into_owned()
    }
}

/// A built tree plus a flat file list for filtering.
#[derive(Debug, Clone, Default)]
pub struct FileTree {
    pub roots: Vec<Arc<FileNode>>,
    pub files: Vec<Arc<FileNode>>,
    pub file_count: usize,
    /// Changes when any path is added or removed, so the UI can skip rebuilding.
    pub signature: u64,
}

#[derive(Default)]
struct Builder {
    dirs: BTreeMap<Vec<u8>, Builder>,
    files: Vec<Vec<u8>>,
}

impl Builder {
    fn insert(&mut self, components: &[&[u8]]) {
        match components {
            [] => {}
            [file] => self.files.push(file.to_vec()),
            [dir, rest @ ..] => self.dirs.entry(dir.to_vec()).or_default().insert(rest),
        }
    }

    fn build(self, prefix: &[u8], files: &mut Vec<Arc<FileNode>>) -> Vec<Arc<FileNode>> {
        let join = |name: &[u8]| {
            if prefix.is_empty() {
                name.to_vec()
            } else {
                let mut path = prefix.to_vec();
                path.push(b'/');
                path.extend_from_slice(name);
                path
            }
        };
        let mut dirs = self
            .dirs
            .into_iter()
            .map(|(name, builder)| {
                let path = join(&name);
                let children = builder.build(&path, files);
                Arc::new(FileNode {
                    name: String::from_utf8_lossy(&name).into_owned(),
                    path,
                    is_dir: true,
                    children,
                })
            })
            .collect::<Vec<_>>();
        dirs.sort_by_cached_key(|node| sort_key(&node.name));
        let mut plain = self
            .files
            .into_iter()
            .map(|name| {
                Arc::new(FileNode {
                    name: String::from_utf8_lossy(&name).into_owned(),
                    path: join(&name),
                    is_dir: false,
                    children: Vec::new(),
                })
            })
            .collect::<Vec<_>>();
        plain.sort_by_cached_key(|node| sort_key(&node.name));
        plain.dedup_by(|a, b| a.path == b.path);
        files.extend(plain.iter().cloned());
        dirs.extend(plain);
        dirs
    }
}

fn sort_key(name: &str) -> (String, String) {
    (name.to_lowercase(), name.to_owned())
}

/// Builds the tree from worktree-relative paths, in any order and with duplicates.
pub fn build_tree(paths: impl IntoIterator<Item = Vec<u8>>) -> FileTree {
    let mut builder = Builder::default();
    let mut sorted = paths
        .into_iter()
        .filter(|path| !path.is_empty() && !path.starts_with(b"/"))
        .collect::<Vec<_>>();
    sorted.sort();
    sorted.dedup();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for path in &sorted {
        path.hash(&mut hasher);
        let components = path
            .split(|byte| *byte == b'/')
            .filter(|part| !part.is_empty() && *part != b"." && *part != b"..")
            .collect::<Vec<_>>();
        builder.insert(&components);
    }
    let mut files = Vec::new();
    let roots = builder.build(b"", &mut files);
    FileTree {
        file_count: files.len(),
        roots,
        files,
        signature: hasher.finish(),
    }
}

/// Files whose path contains every whitespace-separated word of `query`, ignoring case.
/// Basename matches come first so `mod.rs` finds `src/ui/mod.rs` before `src/module/x.rs`.
pub fn filter_files(tree: &FileTree, query: &str) -> (Vec<Arc<FileNode>>, usize) {
    let words = query
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>();
    if words.is_empty() {
        return (Vec::new(), 0);
    }
    let mut by_name = Vec::new();
    let mut by_path = Vec::new();
    for file in &tree.files {
        let path = file.display_path().to_lowercase();
        if !words.iter().all(|word| path.contains(word.as_str())) {
            continue;
        }
        let name = file.name.to_lowercase();
        if words.iter().any(|word| name.contains(word.as_str())) {
            by_name.push(file.clone());
        } else {
            by_path.push(file.clone());
        }
    }
    let total = by_name.len() + by_path.len();
    by_name.extend(by_path);
    by_name.truncate(MAX_FILTER_MATCHES);
    (by_name, total)
}

/// What the tree shows next to each changed path and the folders that contain one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusOverlay {
    pub files: std::collections::HashMap<Vec<u8>, ChangeStatus>,
    pub dirs: HashSet<Vec<u8>>,
}

impl StatusOverlay {
    /// Uses the "All changes" comparison so committed, staged, unstaged and untracked show.
    pub fn from_snapshot(snapshot: &ChangesSnapshot) -> Self {
        let mut overlay = Self::default();
        for file in &snapshot.all_changes {
            let Some(path) = file.new_path.as_ref().or(file.old_path.as_ref()) else {
                continue;
            };
            overlay.files.insert(path.clone(), file.status);
            let mut end = path.len();
            while let Some(slash) = path[..end].iter().rposition(|byte| *byte == b'/') {
                overlay.dirs.insert(path[..slash].to_vec());
                end = slash;
            }
        }
        overlay
    }
}

pub fn status_glyph(status: ChangeStatus) -> &'static str {
    match status {
        ChangeStatus::Added | ChangeStatus::Untracked => "+",
        ChangeStatus::Deleted => "−",
        ChangeStatus::Renamed => "↪",
        ChangeStatus::Unmerged => "!",
        _ => "•",
    }
}

#[cfg(test)]
mod tests {
    use super::{FileNode, build_tree, filter_files};
    use std::sync::Arc;

    fn paths(tree: &[Arc<FileNode>]) -> Vec<String> {
        tree.iter().map(|node| node.display_path()).collect()
    }

    #[test]
    fn directories_come_first_and_names_sort_case_insensitively() {
        let tree = build_tree([
            b"src/zeta.rs".to_vec(),
            b"README.md".to_vec(),
            b"src/Alpha.rs".to_vec(),
            b"build.rs".to_vec(),
            b"docs/notes/todo.md".to_vec(),
            b"src/ui/mod.rs".to_vec(),
        ]);
        assert_eq!(paths(&tree.roots), ["docs", "src", "build.rs", "README.md"]);
        let src = &tree.roots[1];
        assert!(src.is_dir);
        assert_eq!(paths(&src.children), ["src/ui", "src/Alpha.rs", "src/zeta.rs"]);
        assert_eq!(paths(&src.children[0].children), ["src/ui/mod.rs"]);
        assert_eq!(tree.file_count, 6);
        assert_eq!(
            paths(&tree.files),
            [
                "docs/notes/todo.md",
                "src/ui/mod.rs",
                "src/Alpha.rs",
                "src/zeta.rs",
                "build.rs",
                "README.md"
            ]
        );
    }

    #[test]
    fn duplicates_and_odd_paths_are_dropped_and_the_signature_tracks_the_set() {
        let one = build_tree([b"a.txt".to_vec(), b"a.txt".to_vec(), Vec::new()]);
        let same = build_tree([b"a.txt".to_vec()]);
        let other = build_tree([b"a.txt".to_vec(), b"b.txt".to_vec()]);
        assert_eq!(one.file_count, 1);
        assert_eq!(one.signature, same.signature);
        assert_ne!(one.signature, other.signature);
    }

    #[test]
    fn filter_prefers_basename_matches_and_needs_every_word() {
        let tree = build_tree([
            b"src/ui/mod.rs".to_vec(),
            b"src/module/x.rs".to_vec(),
            b"README.md".to_vec(),
        ]);
        let (matches, total) = filter_files(&tree, "MOD");
        assert_eq!(total, 2);
        assert_eq!(paths(&matches), ["src/ui/mod.rs", "src/module/x.rs"]);
        let (matches, _) = filter_files(&tree, "ui rs");
        assert_eq!(paths(&matches), ["src/ui/mod.rs"]);
        assert_eq!(filter_files(&tree, "   ").1, 0);
    }
}
