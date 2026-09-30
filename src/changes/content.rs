use std::collections::hash_map::DefaultHasher;
use std::ffi::OsStr;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use nix::errno::Errno;
use nix::fcntl::{AtFlags, OFlag, openat, readlinkat};
use nix::sys::stat::{Mode, fstat, fstatat};
use serde::{Deserialize, Serialize};

use super::git::{DiffScope, git_argument, git_path_argument, run_checked};
use super::{ChangeStatus, ChangedFile, DiffDocument, WorktreeContext};

const MAX_TEXT_BYTES: usize = 2 * 1024 * 1024;
const MAX_LINES: usize = 50_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum DiffDocumentResult {
    Text(DiffDocument),
    Placeholder(DiffPlaceholder),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffPlaceholder {
    pub path: String,
    pub reason: String,
    pub byte_size: Option<u64>,
    pub original_label: String,
    pub modified_label: String,
    pub metadata: Vec<String>,
    pub identity: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum FileDocumentResult {
    Text(FileDocument),
    Placeholder(FilePlaceholder),
}

/// One worktree file for the read-only viewer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDocument {
    pub path: String,
    pub text: String,
    pub language: Option<String>,
    pub identity: String,
    pub line_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePlaceholder {
    pub path: String,
    pub reason: String,
    pub byte_size: Option<u64>,
    pub identity: String,
}

/// Reads `path` for the file viewer with the diff limits. Symlinks are followed, so a
/// link opens what it points at, and `display_path` is what the viewer shows.
pub fn read_file_document(path: &Path, display_path: &str) -> Result<FileDocumentResult, String> {
    let file_placeholder = |reason: &str, byte_size: Option<u64>, identity: String| {
        Ok(FileDocumentResult::Placeholder(FilePlaceholder {
            path: display_path.to_owned(),
            reason: reason.to_owned(),
            byte_size,
            identity,
        }))
    };
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return file_placeholder("This file does not exist", None, "missing".to_owned());
        }
        Err(error) => return Err(format!("could not inspect file: {error}")),
    };
    if metadata.is_dir() {
        return file_placeholder("This path is a directory", None, "directory".to_owned());
    }
    if !metadata.is_file() {
        return file_placeholder(
            "This path is not a regular file",
            None,
            "non-regular".to_owned(),
        );
    }
    let size = metadata.len();
    if size > MAX_TEXT_BYTES as u64 {
        return file_placeholder(
            "File is larger than the 2 MiB text limit",
            Some(size),
            format!("large:{size}"),
        );
    }
    let file = File::open(path).map_err(|error| format!("could not open file: {error}"))?;
    let mut bytes = Vec::with_capacity(size as usize);
    file.take(MAX_TEXT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("could not read file: {error}"))?;
    use std::os::unix::fs::PermissionsExt;
    let mode = if metadata.permissions().mode() & 0o111 != 0 {
        "100755"
    } else {
        "100644"
    };
    let identity = disk_identity(&bytes, mode);
    if bytes.len() > MAX_TEXT_BYTES {
        return file_placeholder(
            "File is larger than the 2 MiB text limit",
            Some(bytes.len() as u64),
            identity,
        );
    }
    if bytes.contains(&0) {
        return file_placeholder(
            "Binary content is not shown as text",
            Some(bytes.len() as u64),
            identity,
        );
    }
    let line_count = line_count(&bytes);
    if line_count > MAX_LINES {
        return file_placeholder(
            "File is larger than the 50,000 line limit",
            Some(bytes.len() as u64),
            identity,
        );
    }
    let Ok(text) = String::from_utf8(bytes) else {
        return file_placeholder("Text uses an unsupported encoding", Some(size), identity);
    };
    Ok(FileDocumentResult::Text(FileDocument {
        path: display_path.to_owned(),
        language: language_for_path(display_path),
        text,
        identity,
        line_count,
    }))
}

#[derive(Debug, Clone)]
struct LoadedContent {
    bytes: Vec<u8>,
    mode: Option<String>,
    oid: Option<String>,
    identity: String,
    size: Option<u64>,
    special: Option<String>,
}

enum Source<'a> {
    Empty,
    Blob { mode: String, oid: String },
    Tree { revision: String, path: &'a [u8] },
    Index { path: &'a [u8] },
    Disk { path: &'a [u8] },
}

pub fn read_diff_document(
    context: &WorktreeContext,
    file: &ChangedFile,
) -> Result<DiffDocumentResult, String> {
    if file.status == ChangeStatus::Unmerged {
        return Ok(DiffDocumentResult::Placeholder(DiffPlaceholder {
            path: display_changed_path(file),
            reason: "This file has unresolved merge conflicts".to_owned(),
            byte_size: None,
            original_label: "Conflict".to_owned(),
            modified_label: "Conflict".to_owned(),
            metadata: Vec::new(),
            identity: format!("conflict:{}", display_changed_path(file)),
        }));
    }

    let (original_source, modified_source, original_label, modified_label) =
        sources_for(context, file)?;
    let original_source = prefer_captured_blob(
        original_source,
        file.old_mode.as_ref(),
        file.old_blob.as_ref(),
    );
    let modified_source = prefer_captured_blob(
        modified_source,
        file.new_mode.as_ref(),
        file.new_blob.as_ref(),
    );
    let original = load_source(context, original_source, missing_is_expected(file, true))?;
    let modified = load_source(context, modified_source, missing_is_expected(file, false))?;
    let path = display_changed_path(file);
    let mut metadata = Vec::new();
    add_mode_metadata(
        &mut metadata,
        original.mode.as_deref(),
        modified.mode.as_deref(),
    );
    if original.mode.as_deref() == Some("160000") || modified.mode.as_deref() == Some("160000") {
        metadata.push(format!(
            "Submodule object changed: {} to {}",
            original.oid.as_deref().unwrap_or("absent"),
            modified.oid.as_deref().unwrap_or("absent")
        ));
    }

    let identity = document_identity(&original, &modified);
    let special = original
        .special
        .as_ref()
        .or(modified.special.as_ref())
        .cloned();
    if let Some(reason) = special {
        return Ok(DiffDocumentResult::Placeholder(DiffPlaceholder {
            path,
            reason,
            byte_size: combined_size(&original, &modified),
            original_label,
            modified_label,
            metadata,
            identity,
        }));
    }

    if original.bytes.len() > MAX_TEXT_BYTES || modified.bytes.len() > MAX_TEXT_BYTES {
        return Ok(placeholder(
            path,
            "File is larger than the 2 MiB text limit".to_owned(),
            combined_size(&original, &modified),
            original_label,
            modified_label,
            metadata,
            identity,
        ));
    }
    if original.bytes.contains(&0) || modified.bytes.contains(&0) {
        return Ok(placeholder(
            path,
            "Binary content is not shown as text".to_owned(),
            combined_size(&original, &modified),
            original_label,
            modified_label,
            metadata,
            identity,
        ));
    }
    if line_count(&original.bytes) > MAX_LINES || line_count(&modified.bytes) > MAX_LINES {
        return Ok(placeholder(
            path,
            "File is larger than the 50,000 line limit".to_owned(),
            combined_size(&original, &modified),
            original_label,
            modified_label,
            metadata,
            identity,
        ));
    }

    let original_text = match std::str::from_utf8(&original.bytes) {
        Ok(text) => text.to_owned(),
        Err(_) => {
            return Ok(placeholder(
                path,
                "Text uses an unsupported encoding".to_owned(),
                combined_size(&original, &modified),
                original_label,
                modified_label,
                metadata,
                identity,
            ));
        }
    };
    let modified_text = match std::str::from_utf8(&modified.bytes) {
        Ok(text) => text.to_owned(),
        Err(_) => {
            return Ok(placeholder(
                path,
                "Text uses an unsupported encoding".to_owned(),
                combined_size(&original, &modified),
                original_label,
                modified_label,
                metadata,
                identity,
            ));
        }
    };
    if original_text != modified_text
        && normalize_line_endings(&original_text) == normalize_line_endings(&modified_text)
    {
        metadata.push(format!(
            "Only line endings differ: {} to {}",
            line_ending_name(&original_text),
            line_ending_name(&modified_text)
        ));
    }

    Ok(DiffDocumentResult::Text(DiffDocument {
        path: path.clone(),
        original_label,
        modified_label,
        original: original_text,
        modified: modified_text,
        language: language_for_path(&path),
        identity,
        metadata,
    }))
}

pub(super) fn capture_file_metadata(
    context: &WorktreeContext,
    file: &mut ChangedFile,
) -> Result<(), String> {
    if file.status == ChangeStatus::Unmerged {
        return Ok(());
    }
    let (old_metadata, new_metadata) = {
        let (original, modified, _, _) = sources_for(context, file)?;
        (
            source_metadata(context, &original)?,
            source_metadata(context, &modified)?,
        )
    };
    if let Some((mode, oid)) = old_metadata {
        file.old_mode = Some(mode);
        file.old_blob = Some(oid);
    }
    if let Some((mode, oid)) = new_metadata {
        file.new_mode = Some(mode);
        file.new_blob = Some(oid);
    }
    Ok(())
}

fn source_metadata(
    context: &WorktreeContext,
    source: &Source<'_>,
) -> Result<Option<(String, String)>, String> {
    match source {
        Source::Blob { mode, oid } => Ok(Some((mode.clone(), oid.clone()))),
        Source::Tree { revision, path } => {
            Ok(tree_entry(&context.root, revision, path)?.map(|(mode, _, oid)| (mode, oid)))
        }
        Source::Index { path } => match index_entry(&context.root, path)? {
            IndexLookup::Blob { mode, oid } if !oid.bytes().all(|byte| byte == b'0') => {
                Ok(Some((mode, oid)))
            }
            IndexLookup::Missing | IndexLookup::Conflict | IndexLookup::Blob { .. } => Ok(None),
        },
        Source::Empty | Source::Disk { .. } => Ok(None),
    }
}

fn prefer_captured_blob<'a>(
    source: Source<'a>,
    mode: Option<&String>,
    oid: Option<&String>,
) -> Source<'a> {
    match (source, mode, oid) {
        (Source::Tree { .. } | Source::Index { .. }, Some(mode), Some(oid)) => Source::Blob {
            mode: mode.clone(),
            oid: oid.clone(),
        },
        (source, _, _) => source,
    }
}

fn sources_for<'a>(
    context: &'a WorktreeContext,
    file: &'a ChangedFile,
) -> Result<(Source<'a>, Source<'a>, String, String), String> {
    let old_path = file
        .old_path
        .as_deref()
        .or(file.new_path.as_deref())
        .ok_or_else(|| "changed file has no path".to_owned())?;
    let new_path = file
        .new_path
        .as_deref()
        .or(file.old_path.as_deref())
        .ok_or_else(|| "changed file has no path".to_owned())?;
    let empty_old = file.old_path.is_none() || file.status == ChangeStatus::Added;
    let empty_new = file.new_path.is_none() || file.status == ChangeStatus::Deleted;

    let result = match &file.scope {
        DiffScope::All => {
            if context.unborn {
                (
                    Source::Empty,
                    Source::Disk { path: new_path },
                    "Empty".to_owned(),
                    "Working tree".to_owned(),
                )
            } else {
                let merge_base = context.merge_base_oid.as_deref().ok_or_else(|| {
                    context
                        .comparison_warning
                        .clone()
                        .unwrap_or_else(|| "No comparison base is available".to_owned())
                })?;
                (
                    if empty_old {
                        Source::Empty
                    } else {
                        Source::Tree {
                            revision: merge_base.to_owned(),
                            path: old_path,
                        }
                    },
                    if empty_new {
                        Source::Empty
                    } else {
                        Source::Disk { path: new_path }
                    },
                    format!(
                        "Merge base of {}",
                        context.base_ref.as_deref().unwrap_or("base")
                    ),
                    "Working tree".to_owned(),
                )
            }
        }
        DiffScope::Staged => (
            if context.unborn || empty_old {
                Source::Empty
            } else {
                Source::Tree {
                    revision: context
                        .head_oid
                        .as_deref()
                        .ok_or_else(|| "HEAD is unavailable for this staged comparison".to_owned())?
                        .to_owned(),
                    path: old_path,
                }
            },
            if empty_new {
                Source::Empty
            } else {
                Source::Index { path: new_path }
            },
            if context.unborn { "Empty" } else { "HEAD" }.to_owned(),
            "Index".to_owned(),
        ),
        DiffScope::Unstaged => (
            if empty_old {
                Source::Empty
            } else {
                Source::Index { path: old_path }
            },
            if empty_new {
                Source::Empty
            } else {
                Source::Disk { path: new_path }
            },
            "Index".to_owned(),
            "Working tree".to_owned(),
        ),
        DiffScope::Untracked => (
            Source::Empty,
            Source::Disk { path: new_path },
            "Empty".to_owned(),
            "Working tree".to_owned(),
        ),
        DiffScope::Committed => {
            let merge_base = context.merge_base_oid.as_deref().ok_or_else(|| {
                context
                    .comparison_warning
                    .clone()
                    .unwrap_or_else(|| "No comparison base is available".to_owned())
            })?;
            let head = context
                .head_oid
                .as_deref()
                .ok_or_else(|| "HEAD is unavailable for this comparison".to_owned())?;
            (
                if empty_old {
                    Source::Empty
                } else {
                    Source::Tree {
                        revision: merge_base.to_owned(),
                        path: old_path,
                    }
                },
                if empty_new {
                    Source::Empty
                } else {
                    Source::Tree {
                        revision: head.to_owned(),
                        path: new_path,
                    }
                },
                format!(
                    "Merge base of {}",
                    context.base_ref.as_deref().unwrap_or("base")
                ),
                "HEAD".to_owned(),
            )
        }
        DiffScope::Commit { oid } => {
            validate_oid(oid)?;
            let parents = run_checked(
                &context.root,
                [
                    git_argument("rev-list"),
                    git_argument("--parents"),
                    git_argument("-n"),
                    git_argument("1"),
                    git_argument("--end-of-options"),
                    git_argument(oid),
                ],
            )?;
            let fields = parents
                .stdout
                .split(|byte| byte.is_ascii_whitespace())
                .filter(|field| !field.is_empty())
                .collect::<Vec<_>>();
            if fields.first().is_none_or(|commit| !valid_oid_bytes(commit)) {
                return Err("Git returned an invalid commit identity".to_owned());
            }
            let parent = fields
                .get(1)
                .copied()
                .filter(|parent| valid_oid_bytes(parent));
            let commit_oid = std::str::from_utf8(fields[0])
                .map_err(|_| "Git returned an invalid commit identity".to_owned())?
                .to_owned();
            let parent_oid = parent
                .map(std::str::from_utf8)
                .transpose()
                .map_err(|_| "Git returned an invalid parent identity".to_owned())?
                .map(str::to_owned);
            (
                if empty_old || parent_oid.is_none() {
                    Source::Empty
                } else {
                    Source::Tree {
                        revision: parent_oid.clone().expect("checked above"),
                        path: old_path,
                    }
                },
                if empty_new {
                    Source::Empty
                } else {
                    Source::Tree {
                        revision: commit_oid.clone(),
                        path: new_path,
                    }
                },
                parent_oid
                    .as_deref()
                    .map(|parent| format!("Parent {}", short_oid(parent)))
                    .unwrap_or_else(|| "Empty".to_owned()),
                format!("Commit {}", short_oid(&commit_oid)),
            )
        }
    };
    Ok(result)
}

fn load_source(
    context: &WorktreeContext,
    source: Source<'_>,
    missing_is_expected: bool,
) -> Result<LoadedContent, String> {
    match source {
        Source::Empty => Ok(empty_content()),
        Source::Blob { mode, oid } if mode == "160000" => Ok(special_content(
            mode,
            oid,
            "Git submodule object changes are shown without opening submodule content",
        )),
        Source::Blob { mode, oid } => load_blob(&context.root, &mode, &oid),
        Source::Tree { revision, path } => {
            let Some((mode, object_type, oid)) = tree_entry(&context.root, &revision, path)? else {
                return missing_content(missing_is_expected, "commit tree");
            };
            if mode == "160000" || object_type == "commit" {
                return Ok(special_content(
                    mode,
                    oid,
                    "Git submodule object changes are shown without opening submodule content",
                ));
            }
            load_blob(&context.root, &mode, &oid)
        }
        Source::Index { path } => match index_entry(&context.root, path)? {
            IndexLookup::Missing => missing_content(missing_is_expected, "index"),
            IndexLookup::Conflict => Ok(special_content(
                "unmerged",
                String::new(),
                "This file has unresolved merge conflicts",
            )),
            IndexLookup::Blob { mode: _, oid } if oid.bytes().all(|byte| byte == b'0') => {
                Ok(empty_content())
            }
            IndexLookup::Blob { mode, oid } if mode == "160000" => Ok(special_content(
                mode,
                oid,
                "Git submodule object changes are shown without opening submodule content",
            )),
            IndexLookup::Blob { mode, oid } => load_blob(&context.root, &mode, &oid),
        },
        Source::Disk { path } => read_disk_file(&context.root, path, missing_is_expected),
    }
}

fn tree_entry(
    root: &Path,
    revision: &str,
    path: &[u8],
) -> Result<Option<(String, String, String)>, String> {
    validate_oid(revision)?;
    let output = run_checked(
        root,
        [
            git_argument("ls-tree"),
            git_argument("-z"),
            git_argument("--full-tree"),
            git_argument(revision),
            git_argument("--"),
            git_path_argument(path),
        ],
    )?;
    for record in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        let Some(tab) = record.iter().position(|byte| *byte == b'\t') else {
            return Err("Git returned a malformed tree entry".to_owned());
        };
        if &record[tab + 1..] != path {
            continue;
        }
        let fields = record[..tab]
            .split(|byte| *byte == b' ')
            .collect::<Vec<_>>();
        if fields.len() != 3 || !valid_oid_bytes(fields[2]) {
            return Err("Git returned a malformed tree entry".to_owned());
        }
        let mode = std::str::from_utf8(fields[0])
            .map_err(|_| "Git returned an invalid file mode".to_owned())?
            .to_owned();
        let object_type = std::str::from_utf8(fields[1])
            .map_err(|_| "Git returned an invalid tree object type".to_owned())?
            .to_owned();
        let oid = std::str::from_utf8(fields[2])
            .map_err(|_| "Git returned an invalid object ID".to_owned())?
            .to_owned();
        return Ok(Some((mode, object_type, oid)));
    }
    Ok(None)
}

enum IndexLookup {
    Missing,
    Conflict,
    Blob { mode: String, oid: String },
}

fn index_entry(root: &Path, path: &[u8]) -> Result<IndexLookup, String> {
    let output = run_checked(
        root,
        [
            git_argument("ls-files"),
            git_argument("--stage"),
            git_argument("-z"),
            git_argument("--"),
            git_path_argument(path),
        ],
    )?;
    let mut stage_zero = None;
    let mut conflict = false;
    for record in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        let Some(tab) = record.iter().position(|byte| *byte == b'\t') else {
            return Err("Git returned a malformed index entry".to_owned());
        };
        if &record[tab + 1..] != path {
            continue;
        }
        let fields = record[..tab]
            .split(|byte| *byte == b' ')
            .collect::<Vec<_>>();
        if fields.len() != 3 || !valid_oid_bytes(fields[1]) {
            return Err("Git returned a malformed index entry".to_owned());
        }
        let stage = fields[2];
        if stage == b"0" {
            stage_zero = Some((fields[0].to_vec(), fields[1].to_vec()));
        } else {
            conflict = true;
        }
    }
    if conflict && stage_zero.is_none() {
        return Ok(IndexLookup::Conflict);
    }
    let Some((mode, oid)) = stage_zero else {
        return Ok(IndexLookup::Missing);
    };
    Ok(IndexLookup::Blob {
        mode: String::from_utf8(mode)
            .map_err(|_| "Git returned an invalid file mode".to_owned())?,
        oid: String::from_utf8(oid).map_err(|_| "Git returned an invalid object ID".to_owned())?,
    })
}

fn load_blob(root: &Path, mode: &str, oid: &str) -> Result<LoadedContent, String> {
    validate_oid(oid)?;
    let size_output = run_checked(
        root,
        [
            git_argument("cat-file"),
            git_argument("-s"),
            git_argument(oid),
        ],
    )?;
    let size_text = std::str::from_utf8(&size_output.stdout)
        .map_err(|_| "Git returned an invalid blob size".to_owned())?
        .trim();
    let size = size_text
        .parse::<u64>()
        .map_err(|_| "Git returned an invalid blob size".to_owned())?;
    if size > MAX_TEXT_BYTES as u64 {
        return Ok(LoadedContent {
            bytes: Vec::new(),
            mode: Some(mode.to_owned()),
            oid: Some(oid.to_owned()),
            identity: format!("blob:{oid}:{mode}"),
            size: Some(size),
            special: Some("File is larger than the 2 MiB text limit".to_owned()),
        });
    }
    let output = run_checked(
        root,
        [
            git_argument("cat-file"),
            git_argument("blob"),
            git_argument(oid),
        ],
    )?;
    if output.stdout.len() as u64 != size {
        return Err("Git blob size changed while it was being read".to_owned());
    }
    Ok(LoadedContent {
        identity: format!("blob:{oid}:{mode}"),
        bytes: output.stdout,
        mode: Some(mode.to_owned()),
        oid: Some(oid.to_owned()),
        size: Some(size),
        special: None,
    })
}

fn read_disk_file(
    root: &Path,
    raw_path: &[u8],
    missing_is_expected: bool,
) -> Result<LoadedContent, String> {
    let (parent, name) = match open_parent(root, raw_path)? {
        Some(value) => value,
        None => return missing_content(missing_is_expected, "working tree"),
    };
    let stat = match fstatat(&parent, name.as_os_str(), AtFlags::AT_SYMLINK_NOFOLLOW) {
        Ok(stat) => stat,
        Err(Errno::ENOENT) => return missing_content(missing_is_expected, "working tree"),
        Err(error) => return Err(format!("could not inspect worktree file: {error}")),
    };
    let file_type = stat.st_mode & libc::S_IFMT;
    if file_type == libc::S_IFLNK {
        let target = readlinkat(&parent, name.as_os_str())
            .map_err(|error| format!("could not read symlink target: {error}"))?;
        let bytes = target.as_bytes().to_vec();
        return Ok(LoadedContent {
            identity: disk_identity(&bytes, "120000"),
            size: Some(bytes.len() as u64),
            bytes,
            mode: Some("120000".to_owned()),
            oid: None,
            special: None,
        });
    }
    if file_type != libc::S_IFREG {
        return Ok(special_content(
            "non-regular",
            String::new(),
            "This path is not a regular file",
        ));
    }

    let fd = openat(
        &parent,
        name.as_os_str(),
        OFlag::O_RDONLY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|error| format!("could not open worktree file: {error}"))?;
    let before =
        fstat(&fd).map_err(|error| format!("could not inspect open worktree file: {error}"))?;
    let mode = if before.st_mode & 0o111 != 0 {
        "100755"
    } else {
        "100644"
    };
    let mut reader = File::from(fd).take(MAX_TEXT_BYTES as u64 + 1);
    let mut bytes = Vec::with_capacity((before.st_size.max(0) as usize).min(MAX_TEXT_BYTES + 1));
    reader
        .read_to_end(&mut bytes)
        .map_err(|error| format!("could not read worktree file: {error}"))?;
    let after = fstat(&reader.get_ref())
        .map_err(|error| format!("could not recheck worktree file: {error}"))?;
    let current_path = fstatat(&parent, name.as_os_str(), AtFlags::AT_SYMLINK_NOFOLLOW)
        .map_err(|error| format!("worktree file changed while it was being read: {error}"))?;
    if before.st_dev != after.st_dev
        || before.st_ino != after.st_ino
        || before.st_size != after.st_size
        || before.st_mtime != after.st_mtime
        || before.st_mtime_nsec != after.st_mtime_nsec
        || before.st_ctime != after.st_ctime
        || before.st_ctime_nsec != after.st_ctime_nsec
        || current_path.st_dev != before.st_dev
        || current_path.st_ino != before.st_ino
    {
        return Err("worktree file changed while it was being read".to_owned());
    }
    let too_large = bytes.len() > MAX_TEXT_BYTES;
    Ok(LoadedContent {
        identity: disk_identity(&bytes, mode),
        size: Some(after.st_size.max(0) as u64),
        bytes,
        mode: Some(mode.to_owned()),
        oid: None,
        special: too_large.then(|| "File is larger than the 2 MiB text limit".to_owned()),
    })
}

fn open_parent(
    root: &Path,
    raw_path: &[u8],
) -> Result<Option<(OwnedFd, std::ffi::OsString)>, String> {
    if raw_path.is_empty() || raw_path.contains(&0) || raw_path.first() == Some(&b'/') {
        return Err("Git returned an unsafe worktree path".to_owned());
    }
    let components = raw_path.split(|byte| *byte == b'/').collect::<Vec<_>>();
    if components
        .iter()
        .any(|part| part.is_empty() || *part == b"." || *part == b"..")
    {
        return Err("Git returned an unsafe worktree path".to_owned());
    }
    let Some((name, parents)) = components.split_last() else {
        return Err("Git returned an empty worktree path".to_owned());
    };
    let root_file =
        File::open(root).map_err(|error| format!("could not open worktree root: {error}"))?;
    let mut parent: OwnedFd = root_file.into();
    for component in parents {
        let component = OsStr::from_bytes(component);
        match openat(
            &parent,
            component,
            OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
            Mode::empty(),
        ) {
            Ok(next) => parent = next,
            Err(Errno::ENOENT) => return Ok(None),
            Err(error) => {
                return Err(format!(
                    "worktree path has an unsafe or unreadable parent: {error}"
                ));
            }
        }
    }
    Ok(Some((parent, OsStr::from_bytes(name).to_os_string())))
}

fn missing_content(expected: bool, side: &str) -> Result<LoadedContent, String> {
    if expected {
        Ok(empty_content())
    } else {
        Err(format!("{side} file is missing or stale"))
    }
}

fn empty_content() -> LoadedContent {
    LoadedContent {
        bytes: Vec::new(),
        mode: None,
        oid: None,
        identity: "empty".to_owned(),
        size: Some(0),
        special: None,
    }
}

fn special_content(mode: impl Into<String>, oid: impl Into<String>, reason: &str) -> LoadedContent {
    let mode = mode.into();
    let oid = oid.into();
    LoadedContent {
        bytes: Vec::new(),
        identity: format!("special:{mode}:{oid}"),
        mode: Some(mode),
        oid: Some(oid),
        size: None,
        special: Some(reason.to_owned()),
    }
}

fn placeholder(
    path: String,
    reason: String,
    byte_size: Option<u64>,
    original_label: String,
    modified_label: String,
    metadata: Vec<String>,
    identity: String,
) -> DiffDocumentResult {
    DiffDocumentResult::Placeholder(DiffPlaceholder {
        path,
        reason,
        byte_size,
        original_label,
        modified_label,
        metadata,
        identity,
    })
}

fn missing_is_expected(file: &ChangedFile, original: bool) -> bool {
    if original {
        file.old_path.is_none()
            || file.status == ChangeStatus::Added
            || file.status == ChangeStatus::Untracked
    } else {
        file.new_path.is_none() || file.status == ChangeStatus::Deleted
    }
}

pub fn display_changed_path(file: &ChangedFile) -> String {
    let raw = file
        .new_path
        .as_deref()
        .or(file.old_path.as_deref())
        .unwrap_or_default();
    escape_path(raw)
}

fn escape_path(raw: &[u8]) -> String {
    let mut output = String::new();
    let mut remaining = raw;
    while !remaining.is_empty() {
        match std::str::from_utf8(remaining) {
            Ok(valid) => {
                append_display_chars(&mut output, valid);
                break;
            }
            Err(error) => {
                let valid_bytes = error.valid_up_to();
                if valid_bytes > 0 {
                    let valid = std::str::from_utf8(&remaining[..valid_bytes])
                        .expect("Utf8Error::valid_up_to contains valid UTF-8");
                    append_display_chars(&mut output, valid);
                    remaining = &remaining[valid_bytes..];
                }
                let invalid_bytes = error.error_len().unwrap_or(remaining.len()).max(1);
                for byte in &remaining[..invalid_bytes.min(remaining.len())] {
                    output.push_str(&format!("\\x{byte:02X}"));
                }
                remaining = &remaining[invalid_bytes.min(remaining.len())..];
            }
        }
    }
    output
}

fn append_display_chars(output: &mut String, valid: &str) {
    for character in valid.chars() {
        if character == '\\' {
            output.push_str("\\\\");
        } else if character.is_control() {
            output.push_str(&format!("\\u{{{:04X}}}", u32::from(character)));
        } else {
            output.push(character);
        }
    }
}

fn language_for_path(path: &str) -> Option<String> {
    let filename = path.rsplit(['/', '\\']).next()?.to_ascii_lowercase();
    match filename.as_str() {
        ".bashrc" | ".zshrc" | ".zshenv" | ".profile" => return Some("shell".to_owned()),
        "dockerfile" | "containerfile" => return Some("dockerfile".to_owned()),
        // Lock files are TOML, which the INI grammar colors well enough.
        "cargo.lock" => return Some("ini".to_owned()),
        _ => {}
    }
    let extension = filename.rsplit_once('.')?.1;
    let language = match extension {
        "c" | "cc" | "cpp" | "cxx" | "h" | "hpp" | "hxx" => "cpp",
        "cs" => "csharp",
        "css" => "css",
        "go" => "go",
        "html" | "htm" => "html",
        "ini" | "toml" | "cfg" | "conf" | "env" => "ini",
        "java" => "java",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "json" | "jsonc" | "json5" => "json",
        "md" => "markdown",
        "mdx" => "mdx",
        "php" => "php",
        "py" | "pyi" => "python",
        "rb" => "ruby",
        "rs" => "rust",
        "scss" => "scss",
        "sh" | "bash" | "zsh" => "shell",
        "sql" => "sql",
        "svg" | "xml" | "xsl" | "csproj" | "props" | "targets" => "xml",
        "ts" | "tsx" | "mts" | "cts" => "typescript",
        "yaml" | "yml" => "yaml",
        _ => return None,
    };
    Some(language.to_owned())
}

fn add_mode_metadata(metadata: &mut Vec<String>, original: Option<&str>, modified: Option<&str>) {
    if let (Some(original), Some(modified)) = (original, modified)
        && original != modified
    {
        metadata.push(format!("Mode changed: {original} to {modified}"));
        if (original == "100644" && modified == "100755")
            || (original == "100755" && modified == "100644")
        {
            metadata.push("Executable permission changed".to_owned());
        }
    }
}

fn document_identity(original: &LoadedContent, modified: &LoadedContent) -> String {
    format!("{}=>{}", original.identity, modified.identity)
}

fn disk_identity(bytes: &[u8], mode: &str) -> String {
    let mut hasher = DefaultHasher::new();
    mode.hash(&mut hasher);
    bytes.hash(&mut hasher);
    format!("disk:{mode}:{}:{:016x}", bytes.len(), hasher.finish())
}

fn combined_size(original: &LoadedContent, modified: &LoadedContent) -> Option<u64> {
    Some(original.size?.saturating_add(modified.size?))
}

fn line_count(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        return 1;
    }
    let newlines = bytes.iter().filter(|byte| **byte == b'\n').count();
    if bytes.last() == Some(&b'\n') {
        newlines.max(1)
    } else {
        newlines + 1
    }
}

fn normalize_line_endings(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn line_ending_name(text: &str) -> &'static str {
    let crlf = text.contains("\r\n");
    let standalone_lf = text.as_bytes().windows(1).any(|bytes| bytes == b"\n")
        && !text.replace("\r\n", "").contains('\n');
    let standalone_cr = text.replace("\r\n", "").contains('\r');
    match (crlf, standalone_lf, standalone_cr) {
        (true, false, false) => "CRLF",
        (false, true, false) => "LF",
        (false, false, true) => "CR",
        _ => "mixed line endings",
    }
}

fn validate_oid(oid: &str) -> Result<(), String> {
    if (oid.len() == 40 || oid.len() == 64) && oid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("Git returned an invalid object ID".to_owned())
    }
}

fn valid_oid_bytes(oid: &[u8]) -> bool {
    (oid.len() == 40 || oid.len() == 64) && oid.iter().all(u8::is_ascii_hexdigit)
}

fn short_oid(oid: &str) -> &str {
    &oid[..oid.len().min(12)]
}
