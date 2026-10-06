use agtk::projects::{ProjectPreferences, repository_url};

fn git(root: &std::path::Path, args: &[&str]) {
    assert!(
        std::process::Command::new("git")
            .current_dir(root)
            .args(args)
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn repository_links_convert_github_and_azure_clone_urls() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    git(root, &["init", "-q"]);
    for (remote, expected) in [
        (
            "https://github.com/owner/repo.git",
            "https://github.com/owner/repo",
        ),
        (
            "git@github.com:owner/repo.git",
            "https://github.com/owner/repo",
        ),
        (
            "ssh://git@github.com/owner/repo.git",
            "https://github.com/owner/repo",
        ),
        (
            "ssh://git@ssh.github.com:443/owner/repo.git",
            "https://github.com/owner/repo",
        ),
        (
            "https://user:secret@github.com/owner/repo.git?token=secret#ref",
            "https://github.com/owner/repo",
        ),
        (
            "https://org@dev.azure.com/org/Project%20One/_git/repo",
            "https://dev.azure.com/org/Project%20One/_git/repo",
        ),
        (
            "git@ssh.dev.azure.com:v3/org/Project%20One/repo",
            "https://dev.azure.com/org/Project%20One/_git/repo",
        ),
        (
            "ssh://git@ssh.dev.azure.com:22/v3/org/Project%20One/repo",
            "https://dev.azure.com/org/Project%20One/_git/repo",
        ),
        (
            "git@vs-ssh.visualstudio.com:v3/org/project/repo",
            "https://dev.azure.com/org/project/_git/repo",
        ),
        (
            "https://org.visualstudio.com/DefaultCollection/project/_git/repo",
            "https://org.visualstudio.com/DefaultCollection/project/_git/repo",
        ),
        (
            "http://server:8080/tfs/collection/project/_git/repo",
            "http://server:8080/tfs/collection/project/_git/repo",
        ),
    ] {
        git(root, &["config", "remote.upstream.url", remote]);
        assert_eq!(repository_url(root).as_deref(), Some(expected), "{remote}");
    }
}

#[test]
fn repository_links_use_the_first_remote_without_transport_rewrites() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    git(root, &["init", "-q"]);
    git(
        root,
        &[
            "remote",
            "add",
            "z-last",
            "https://github.com/owner/last.git",
        ],
    );
    git(
        root,
        &[
            "remote",
            "add",
            "a-first",
            "https://github.com/owner/first.git",
        ],
    );
    git(
        root,
        &[
            "config",
            "url.ssh://git@private-alias/.insteadOf",
            "https://github.com/",
        ],
    );
    assert_eq!(
        repository_url(root).as_deref(),
        Some("https://github.com/owner/first")
    );
}

#[test]
fn repository_links_hide_missing_local_or_unsupported_remotes() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    assert_eq!(repository_url(root), None);
    git(root, &["init", "-q"]);
    assert_eq!(repository_url(root), None);
    for remote in [
        "/tmp/repo.git",
        "../repo.git",
        "file:///tmp/repo.git",
        "javascript:alert(1)",
        "",
        "https://github.com/",
        "git@ssh.dev.azure.com:v3/org/project",
        "git@ssh.dev.azure.com:v3/org//repo",
    ] {
        git(root, &["config", "remote.origin.url", remote]);
        assert_eq!(repository_url(root), None, "{remote}");
    }
}

#[test]
fn project_preferences_persist_pin_and_collapse_independently() {
    let mut preferences = ProjectPreferences::default();
    preferences.set("/work/agtk", Some(true), None);
    preferences.set("/work/agtk", None, Some(true));

    let project = preferences.get("/work/agtk");
    assert!(project.is_pinned);
    assert!(project.is_collapsed);
    assert!(!preferences.get("/work/other").is_pinned);

    let json = serde_json::to_value(&preferences).expect("serialize projects");
    assert_eq!(json["projects"]["/work/agtk"]["isPinned"], true);
    assert_eq!(
        serde_json::from_value::<ProjectPreferences>(json).expect("deserialize projects"),
        preferences
    );
}

#[test]
fn a_remembered_project_stays_listed_until_it_is_removed() {
    let mut preferences = ProjectPreferences::default();
    assert!(preferences.remember("/work/agtk"));
    assert!(!preferences.remember("/work/agtk"));

    preferences.set("/work/agtk", Some(true), Some(true));
    preferences.set("/work/agtk", Some(false), Some(false));
    assert!(preferences.projects.contains_key("/work/agtk"));

    assert!(preferences.remove("/work/agtk"));
    assert!(!preferences.remove("/work/agtk"));
    assert!(preferences.projects.is_empty());
}

#[test]
fn a_remembered_project_saves_in_the_settings_format_older_builds_read() {
    let mut preferences = ProjectPreferences::default();
    preferences.remember("/work/agtk");
    assert_eq!(
        serde_json::to_value(&preferences).expect("serialize projects"),
        serde_json::json!({"projects":{"/work/agtk":{"isPinned":false,"isCollapsed":false}}})
    );
}
