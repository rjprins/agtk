use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::control::CaptureResult;
use crate::instance::ensure_private_dir;

const CAPTURE_RETENTION: usize = 20;
static CAPTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub fn store_png_capture(
    directory: &Path,
    png: &[u8],
    width: i32,
    height: i32,
) -> io::Result<CaptureResult> {
    ensure_private_dir(directory)?;

    let path = unique_capture_path(directory);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    if let Err(error) = file.write_all(png) {
        let _ = fs::remove_file(&path);
        return Err(error);
    }
    drop(file);

    prune_old_captures(directory)?;
    let sha256 = glib::compute_checksum_for_data(glib::ChecksumType::Sha256, png)
        .ok_or_else(|| io::Error::other("GLib could not calculate a SHA-256 digest"))?
        .to_string();
    Ok(CaptureResult {
        path,
        width,
        height,
        sha256,
    })
}

fn unique_capture_path(directory: &Path) -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = CAPTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    directory.join(format!(
        "capture-{timestamp}-{}-{sequence}.png",
        std::process::id()
    ))
}

fn prune_old_captures(directory: &Path) -> io::Result<()> {
    let mut captures = fs::read_dir(directory)?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().is_ok_and(|kind| kind.is_file())
                && entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with("capture-") && name.ends_with(".png"))
        })
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    captures.sort();
    let remove_count = captures.len().saturating_sub(CAPTURE_RETENTION);
    for path in captures.into_iter().take(remove_count) {
        fs::remove_file(path)?;
    }
    Ok(())
}
