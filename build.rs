use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let viewer = root.join("viewer");
    let dist = viewer.join("dist");
    let stamp = dist.join(".agmux-viewer-build");
    let source_files = [
        viewer.join("index.html"),
        viewer.join("vite.config.js"),
        viewer.join("package.json"),
        viewer.join("package-lock.json"),
    ];

    for path in &source_files {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    println!("cargo:rerun-if-changed={}", viewer.join("src").display());
    println!("cargo:rerun-if-changed={}", stamp.display());

    let stamp_time = fs::metadata(&stamp)
        .and_then(|metadata| metadata.modified())
        .unwrap_or_else(|_| {
            panic!(
                "The embedded Changes viewer is missing. Run scripts/build-viewer.sh before building agmux-native."
            )
        });
    for path in source_files
        .iter()
        .chain(walk_files(&viewer.join("src")).iter())
    {
        if modified(path).is_some_and(|time| time > stamp_time) {
            panic!(
                "The embedded Changes viewer is stale because {} changed. Run scripts/build-viewer.sh before building agmux-native.",
                path.display()
            );
        }
    }

    let index = dist.join("index.html");
    if !index.is_file() {
        panic!(
            "The embedded Changes viewer is missing. Run scripts/build-viewer.sh before building agmux-native."
        );
    }

    let mut files = walk_files(&dist);
    files.retain(|path| path != &stamp);
    files.sort();
    let manifest = std::env::var_os("OUT_DIR")
        .map(PathBuf::from)
        .expect("Cargo did not provide OUT_DIR")
        .join("viewer.gresource.xml");
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<gresources>\n  <gresource prefix=\"/nl/rutger/AgmuxNative/viewer\">\n",
    );
    for path in files {
        let alias = path
            .strip_prefix(&dist)
            .expect("viewer asset must be inside dist")
            .to_string_lossy()
            .replace('\\', "/");
        let alias = xml_escape(&alias);
        xml.push_str(&format!("    <file alias=\"{alias}\">{alias}</file>\n"));
    }
    xml.push_str("  </gresource>\n</gresources>\n");
    fs::write(&manifest, xml).expect("could not write generated viewer resource manifest");

    glib_build_tools::compile_resources(
        &[&dist],
        manifest.to_str().expect("resource path must be UTF-8"),
        "viewer.gresource",
    );
}

fn modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
}

fn walk_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = fs::read_dir(root) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(walk_files(&path));
        } else if path.is_file() {
            files.push(path);
        }
    }
    files
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
