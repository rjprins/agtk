use std::fs;
use std::os::unix::fs::PermissionsExt;

use agmux_native::capture::store_png_capture;

#[test]
fn capture_store_creates_unique_private_files_with_sha256_metadata() {
    let directory = tempfile::tempdir().expect("create capture root");
    let png = b"\x89PNG\r\n\x1a\nfixture";

    let first = store_png_capture(directory.path(), png, 1200, 800).expect("store first capture");
    let second = store_png_capture(directory.path(), png, 1200, 800).expect("store second capture");

    assert_ne!(first.path, second.path);
    assert_eq!(first.width, 1200);
    assert_eq!(first.height, 800);
    assert_eq!(first.sha256, second.sha256);
    assert_eq!(first.sha256.len(), 64);
    assert_eq!(fs::read(&first.path).expect("read capture"), png);
    assert_eq!(
        fs::metadata(&first.path)
            .expect("capture metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn capture_store_retains_only_the_twenty_newest_captures() {
    let directory = tempfile::tempdir().expect("create capture root");

    for value in 0..22_u8 {
        store_png_capture(directory.path(), &[value], 1, 1).expect("store capture");
    }

    let captures = fs::read_dir(directory.path())
        .expect("read capture directory")
        .collect::<Result<Vec<_>, _>>()
        .expect("read capture entries");
    assert_eq!(captures.len(), 20);
}
