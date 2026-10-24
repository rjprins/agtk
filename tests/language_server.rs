use std::fs;
use std::path::Path;
use std::sync::Arc;

use agtk::language_server::{LanguageServers, Lookup, LookupResult, Query};

fn installed(command: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(command).is_file()))
}

fn look_up(
    servers: &LanguageServers,
    lookup: Lookup,
    root: &Path,
    path: &Path,
    text: &str,
    line: u32,
    character: u32,
) -> LookupResult {
    let receiver = servers.lookup(Query {
        lookup,
        root: root.to_owned(),
        path: path.to_owned(),
        text: Arc::from(text),
        line,
        character,
    });
    glib::MainContext::new()
        .block_on(receiver)
        .unwrap_or_else(|_| Err("server thread stopped".to_owned()))
}

#[test]
fn pyright_finds_definitions_and_references_across_files() {
    if !installed("pyright-langserver") && !installed("basedpyright-langserver") {
        eprintln!("skipped: pyright is not installed");
        return;
    }
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().canonicalize().unwrap();
    let helpers = root.join("helpers.py");
    let main = root.join("main.py");
    fs::write(&helpers, "def greet(name):\n    return f\"Hello {name}\"\n").unwrap();
    let text = "from helpers import greet\n\nprint(greet(\"you\"))\n";
    fs::write(&main, text).unwrap();
    let servers = LanguageServers::default();

    let definition = look_up(&servers, Lookup::Definition, &root, &main, text, 2, 7).unwrap();
    let location = definition
        .locations
        .iter()
        .find(|location| location.path == helpers)
        .expect("definition in helpers.py");
    assert_eq!(location.start, (0, 4));
    assert!(!location.in_query);
    assert!(definition.files[&helpers].starts_with("def greet"));

    // The shown text wins over the file on disk.
    let shifted = format!("\n\n{text}");
    let definition = look_up(&servers, Lookup::Definition, &root, &main, &shifted, 4, 7).unwrap();
    assert!(
        definition
            .locations
            .iter()
            .any(|location| location.path == helpers)
    );

    let references = look_up(&servers, Lookup::References, &root, &main, &shifted, 4, 7).unwrap();
    assert!(
        references
            .locations
            .iter()
            .any(|location| location.path == helpers)
    );
    assert!(
        references
            .locations
            .iter()
            .any(|location| location.in_query && location.start == (4, 6))
    );
    servers.stop(&root);
}
