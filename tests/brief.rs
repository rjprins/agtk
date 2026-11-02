use agtk::brief::{
    Brief, BriefCommentClear, BriefItemInput, BriefLink, BriefPostParams, BriefSession,
    REMEMBER_MILLIS, SessionTarget,
};
use agtk::providers::AgentProvider;
use serde_json::json;

fn item(id: &str, section: &str, text: &str) -> BriefItemInput {
    BriefItemInput {
        id: id.to_owned(),
        section: section.to_owned(),
        text: text.to_owned(),
        link: None,
        session: None,
    }
}

fn post(items: Vec<BriefItemInput>) -> BriefPostParams {
    BriefPostParams {
        items,
        ..BriefPostParams::default()
    }
}

fn replace(sections: &[&str], items: Vec<BriefItemInput>) -> BriefPostParams {
    BriefPostParams {
        items,
        replace_sections: sections.iter().map(|&section| section.to_owned()).collect(),
        ..BriefPostParams::default()
    }
}

fn shown(brief: &Brief) -> Vec<(String, Vec<String>)> {
    brief
        .visible_sections()
        .into_iter()
        .map(|(section, items)| {
            (
                section.to_owned(),
                items.into_iter().map(|item| item.id.clone()).collect(),
            )
        })
        .collect()
}

fn get<'a>(brief: &'a Brief, id: &str) -> &'a agtk::brief::BriefItem {
    brief.items.iter().find(|item| item.id == id).unwrap()
}

#[test]
fn new_items_are_unread_and_grouped_in_posted_section_order() {
    let mut brief = Brief::default();
    let changes = brief
        .post(
            &post(vec![
                item("next", "Next action", "Review PR 105687"),
                item("todoist:1", "Tasks", "Renew passport"),
                item("cal:standup", "Today", "09:30 standup"),
                item("todoist:2", "Tasks", "Book dentist"),
            ]),
            1_000,
        )
        .unwrap();
    assert_eq!(
        changes.added,
        ["next", "todoist:1", "cal:standup", "todoist:2"]
    );
    assert_eq!(
        shown(&brief),
        [
            ("Next action".to_owned(), vec!["next".to_owned()]),
            (
                "Tasks".to_owned(),
                vec!["todoist:1".to_owned(), "todoist:2".to_owned()]
            ),
            ("Today".to_owned(), vec!["cal:standup".to_owned()]),
        ]
    );
    assert_eq!(brief.unread_count(), 4);
    assert!(brief.items.iter().all(|item| !item.read && !item.hidden));
}

#[test]
fn reposting_an_id_updates_it_in_place_and_keeps_its_marks() {
    let mut brief = Brief::default();
    brief
        .post(
            &post(vec![
                item("ado-pr:1", "Waiting on you", "Review PR 1"),
                item("ado-pr:2", "Waiting on you", "Review PR 2"),
            ]),
            1_000,
        )
        .unwrap();
    assert!(brief.mark_read("ado-pr:1"));
    assert!(brief.hide("ado-pr:2"));

    let mut changed = item("ado-pr:1", "Waiting on you", "Review PR 1 (2 new comments)");
    changed.link = Some("https://dev.azure.com/org/project/_git/repo/pullrequest/1".to_owned());
    let changes = brief
        .post(
            &post(vec![
                changed,
                item("ado-pr:2", "Waiting on you", "Review PR 2"),
            ]),
            2_000,
        )
        .unwrap();
    assert_eq!(changes.updated, ["ado-pr:1"]);
    assert!(changes.added.is_empty());
    let first = get(&brief, "ado-pr:1");
    assert!(first.read, "an update must not re-highlight a read item");
    assert_eq!(first.text, "Review PR 1 (2 new comments)");
    assert_eq!((first.posted_at, first.updated_at), (1_000, 2_000));
    assert!(get(&brief, "ado-pr:2").hidden);
    assert_eq!(
        shown(&brief),
        [("Waiting on you".to_owned(), vec!["ado-pr:1".to_owned()])]
    );
    assert_eq!(brief.unread_count(), 0);
}

#[test]
fn replacing_a_section_uses_the_posted_order_and_retires_the_rest() {
    let mut brief = Brief::default();
    brief
        .post(
            &post(vec![
                item("a", "Today", "A"),
                item("b", "Today", "B"),
                item("t", "Tasks", "T"),
            ]),
            1_000,
        )
        .unwrap();
    brief.mark_read("a");
    let changes = brief
        .post(
            &replace(
                &["Today", "Coming up"],
                vec![item("c", "Today", "C"), item("a", "Today", "A")],
            ),
            2_000,
        )
        .unwrap();
    assert_eq!(changes.added, ["c"]);
    assert_eq!(changes.retired, ["b"]);
    assert_eq!(
        shown(&brief),
        [
            ("Today".to_owned(), vec!["c".to_owned(), "a".to_owned()]),
            ("Tasks".to_owned(), vec!["t".to_owned()]),
        ]
    );
    assert!(get(&brief, "a").read);
    // b was neither read nor hidden, so nothing about it is remembered.
    assert!(brief.retired.is_empty());

    // Posting no items empties a replaced section.
    brief.post(&replace(&["Today"], Vec::new()), 3_000).unwrap();
    assert_eq!(shown(&brief), [("Tasks".to_owned(), vec!["t".to_owned()])]);
}

#[test]
fn retired_marks_come_back_on_a_repost_and_are_forgotten_after_thirty_days() {
    let mut brief = Brief::default();
    brief
        .post(
            &post(vec![
                item("todoist:1", "Tasks", "Renew passport"),
                item("gmail:abc", "Mail", "Invoice from the landlord"),
                item("gmail:def", "Mail", "Newsletter"),
            ]),
            1_000,
        )
        .unwrap();
    brief.hide("todoist:1");
    brief.mark_read("gmail:abc");
    let changes = brief
        .post(
            &BriefPostParams {
                items: Vec::new(),
                replace_sections: vec!["Tasks".to_owned()],
                retire_ids: vec!["gmail:abc".to_owned(), "gmail:def".to_owned()],
                ..BriefPostParams::default()
            },
            2_000,
        )
        .unwrap();
    assert_eq!(changes.retired, ["todoist:1", "gmail:abc", "gmail:def"]);
    assert!(brief.items.is_empty());
    assert_eq!(
        serde_json::to_value(&brief.retired).unwrap(),
        json!([
            {"id":"todoist:1","read":false,"hidden":true,"done":false,"doneAt":null,"comment":null,"commentedAt":null,"retiredAt":2_000},
            {"id":"gmail:abc","read":true,"hidden":false,"done":false,"doneAt":null,"comment":null,"commentedAt":null,"retiredAt":2_000}
        ])
    );

    // The next morning the same task and mail come back with their marks.
    brief
        .post(
            &post(vec![
                item("todoist:1", "Tasks", "Renew passport"),
                item("gmail:abc", "Mail", "Invoice from the landlord"),
                item("gmail:def", "Mail", "Newsletter"),
            ]),
            3_000,
        )
        .unwrap();
    assert!(get(&brief, "todoist:1").hidden);
    assert!(get(&brief, "gmail:abc").read);
    assert!(!get(&brief, "gmail:def").read);
    assert!(brief.retired.is_empty());

    brief.post(&replace(&["Tasks"], Vec::new()), 4_000).unwrap();
    assert_eq!(brief.retired.len(), 1);
    brief
        .post(&post(Vec::new()), 4_000 + REMEMBER_MILLIS)
        .unwrap();
    assert!(brief.retired.is_empty());
    brief
        .post(
            &post(vec![item("todoist:1", "Tasks", "Renew passport")]),
            5_000 + REMEMBER_MILLIS,
        )
        .unwrap();
    assert!(!get(&brief, "todoist:1").hidden);
}

#[test]
fn moving_an_item_into_a_replaced_section_keeps_its_marks() {
    let mut brief = Brief::default();
    brief
        .post(&post(vec![item("x", "Coming up", "Dentist")]), 1_000)
        .unwrap();
    brief.mark_read("x");
    let changes = brief
        .post(
            &replace(&["Today"], vec![item("x", "Today", "Dentist")]),
            2_000,
        )
        .unwrap();
    assert_eq!(changes.updated, ["x"]);
    assert!(changes.retired.is_empty());
    assert!(get(&brief, "x").read);
    assert_eq!(shown(&brief), [("Today".to_owned(), vec!["x".to_owned()])]);
}

#[test]
fn marking_from_the_panel_reports_only_real_changes() {
    let mut brief = Brief::default();
    brief
        .post(
            &post(vec![item("a", "Today", "A"), item("b", "Today", "B")]),
            1_000,
        )
        .unwrap();
    assert!(!brief.mark_read("missing"));
    assert!(!brief.hide("missing"));
    assert!(brief.hide("b"));
    assert!(!brief.hide("b"));
    assert!(brief.mark_read("a"));
    assert!(!brief.mark_read("a"));
    // Hidden items keep their own read mark and do not count as unread.
    assert!(!get(&brief, "b").read);
    assert_eq!(brief.summary()["unreadCount"], 0);
}

#[test]
fn invalid_posts_change_nothing() {
    let mut brief = Brief::default();
    brief
        .post(&post(vec![item("a", "Today", "A")]), 1_000)
        .unwrap();
    let before = brief.clone();
    let mut bad_link = item("b", "Today", "B");
    bad_link.link = Some("file:///etc/passwd".to_owned());
    let invalid = [
        post(vec![item("b", "Today", "B"), item("b", "Today", "B again")]),
        post(vec![bad_link]),
        post(vec![item(" b", "Today", "B")]),
        post(vec![item("b", "", "B")]),
        post(vec![item("b", "Today", "  ")]),
        post(vec![item("b", "Today", "bell\u{7}")]),
        BriefPostParams {
            items: vec![item("b", "Today", "B")],
            replace_sections: Vec::new(),
            retire_ids: vec!["b".to_owned()],
            ..BriefPostParams::default()
        },
        post(
            (0..201)
                .map(|index| item(&format!("i{index}"), "Today", "x"))
                .collect(),
        ),
    ];
    for params in invalid {
        assert!(brief.post(&params, 2_000).is_err(), "{params:?}");
        assert_eq!(brief, before);
    }
    let mut multiline = item("b", "Today", "Line one\nLine two");
    multiline.link = Some("https://mail.google.com/mail/u/0/#inbox/abc".to_owned());
    assert!(brief.post(&post(vec![multiline]), 2_000).is_ok());
}

#[test]
fn a_damaged_saved_brief_loads_empty() {
    assert_eq!(
        Brief::from_value_lossy(json!({"items":"nope"})),
        Brief::default()
    );
    let mut brief = Brief::default();
    brief
        .post(&post(vec![item("a", "Today", "A")]), 1_000)
        .unwrap();
    brief.mark_read("a");
    let saved = serde_json::to_value(&brief).unwrap();
    assert_eq!(Brief::from_value_lossy(saved), brief);
}

#[test]
fn done_toggles_with_its_time_and_also_reads_and_hides() {
    let mut brief = Brief::default();
    brief
        .post(
            &post(vec![item("todoist:7", "Tasks", "Renew passport")]),
            1_000,
        )
        .unwrap();
    assert!(!brief.toggle_done("missing", 2_000));

    assert!(brief.toggle_done("todoist:7", 2_000));
    let done = get(&brief, "todoist:7");
    assert!(done.done && done.read && done.hidden);
    assert_eq!(done.done_at, Some(2_000));
    assert_eq!(brief.unread_count(), 0);
    assert!(shown(&brief).is_empty());
    assert_eq!(brief.hidden_items()[0].id, "todoist:7");
    assert_eq!(brief.summary()["items"][0]["doneAt"], 2_000);

    // Undoing clears the mark and its time and brings the item back, still read.
    assert!(brief.toggle_done("todoist:7", 3_000));
    let undone = get(&brief, "todoist:7");
    assert!(!undone.done && undone.read && !undone.hidden);
    assert_eq!(
        shown(&brief),
        [("Tasks".to_owned(), vec!["todoist:7".to_owned()])]
    );
    assert_eq!(undone.done_at, None);
    assert_eq!(brief.summary()["items"][0]["done"], false);
}

#[test]
fn done_survives_reposts_and_retirement() {
    let mut brief = Brief::default();
    brief
        .post(
            &post(vec![
                item("todoist:7", "Tasks", "Renew passport"),
                item("todoist:8", "Tasks", "Book dentist"),
            ]),
            1_000,
        )
        .unwrap();
    brief.toggle_done("todoist:7", 2_000);
    brief
        .post(
            &replace(
                &["Tasks"],
                vec![item("todoist:7", "Tasks", "Renew passport!")],
            ),
            3_000,
        )
        .unwrap();
    assert_eq!(get(&brief, "todoist:7").done_at, Some(2_000));

    // The assistant completes the task and retires it; the mark is kept.
    brief
        .post(
            &BriefPostParams {
                retire_ids: vec!["todoist:7".to_owned()],
                ..BriefPostParams::default()
            },
            4_000,
        )
        .unwrap();
    assert_eq!(
        serde_json::to_value(&brief.retired).unwrap(),
        json!([{
            "id":"todoist:7","read":true,"hidden":true,
            "done":true,"doneAt":2_000,"comment":null,"commentedAt":null,"retiredAt":4_000
        }])
    );
    brief
        .post(
            &post(vec![item("todoist:7", "Tasks", "Renew passport")]),
            5_000,
        )
        .unwrap();
    let back = get(&brief, "todoist:7");
    assert!(back.done);
    assert_eq!(back.done_at, Some(2_000));
}

#[test]
fn a_brief_saved_before_done_and_hidden_existed_still_loads() {
    let saved = json!({
        "items":[{"id":"a","section":"Today","text":"A","link":null,
                  "postedAt":1,"updatedAt":1,"read":true,"removed":false}],
        "sections":["Today"],
        "retired":[{"id":"b","read":false,"removed":true,"comment":null,"commentedAt":null,"retiredAt":1}]
    });
    let brief = Brief::from_value_lossy(saved);
    assert_eq!(brief.items.len(), 1);
    assert!(brief.items[0].read && !brief.items[0].done);
    assert_eq!(brief.items[0].done_at, None);
    assert!(brief.retired[0].hidden && !brief.retired[0].done);
}

#[test]
fn hidden_items_are_listed_apart_and_can_be_unhidden_in_the_panel() {
    let mut brief = Brief::default();
    brief
        .post(
            &post(vec![
                item("a", "Today", "A"),
                item("b", "Today", "B"),
                item("c", "Tasks", "C"),
            ]),
            1_000,
        )
        .unwrap();
    brief.hide("c");
    brief.hide("a");
    assert_eq!(shown(&brief), [("Today".to_owned(), vec!["b".to_owned()])]);
    let hidden = brief
        .hidden_items()
        .into_iter()
        .map(|item| item.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(hidden, ["a", "c"]);
    // A section that only holds hidden items is still a section of the brief.
    assert_eq!(brief.sections, ["Today", "Tasks"]);
    assert_eq!(brief.summary()["items"][2]["hidden"], true);

    assert!(brief.unhide("a"));
    assert!(!brief.unhide("a"));
    assert!(!brief.unhide("missing"));
    assert_eq!(
        shown(&brief),
        [("Today".to_owned(), vec!["a".to_owned(), "b".to_owned()])]
    );
}

#[test]
fn the_assistant_unhides_items_but_cannot_change_read_or_done() {
    let mut brief = Brief::default();
    brief
        .post(
            &post(vec![
                item("ado-pr:1", "Waiting on you", "Review PR 1"),
                item("gmail:abc", "Mail", "Landlord"),
                item("gmail:def", "Mail", "Newsletter"),
            ]),
            1_000,
        )
        .unwrap();
    brief.mark_read("ado-pr:1");
    brief.hide("ado-pr:1");
    brief.toggle_done("gmail:abc", 1_500);
    brief.hide("gmail:abc");
    brief.hide("gmail:def");
    // gmail:abc and gmail:def leave the brief hidden; their marks are kept.
    brief.post(&replace(&["Mail"], Vec::new()), 2_000).unwrap();
    assert_eq!(brief.retired.len(), 2);

    let changes = brief
        .post(
            &BriefPostParams {
                items: vec![item("ado-pr:1", "Waiting on you", "Review PR 1: new push")],
                unhide_ids: vec![
                    "ado-pr:1".to_owned(),
                    "gmail:abc".to_owned(),
                    "gmail:def".to_owned(),
                    "missing".to_owned(),
                ],
                ..BriefPostParams::default()
            },
            3_000,
        )
        .unwrap();
    assert_eq!(changes.unhidden, ["ado-pr:1", "gmail:abc", "gmail:def"]);
    let pr = get(&brief, "ado-pr:1");
    assert!(!pr.hidden && pr.read, "unhiding leaves read alone");
    // Kept for its done mark; gmail:def had nothing else worth keeping.
    assert_eq!(
        serde_json::to_value(&brief.retired).unwrap(),
        json!([{
            "id":"gmail:abc","read":true,"hidden":false,
            "done":true,"doneAt":1_500,"comment":null,"commentedAt":null,"retiredAt":2_000
        }])
    );
    brief
        .post(&post(vec![item("gmail:def", "Mail", "Newsletter")]), 4_000)
        .unwrap();
    assert!(!get(&brief, "gmail:def").hidden);

    let invalid = BriefPostParams {
        unhide_ids: vec![" bad".to_owned()],
        ..BriefPostParams::default()
    };
    assert!(brief.post(&invalid, 5_000).is_err());
}

#[test]
fn empty_sections_leave_the_brief() {
    let mut brief = Brief::default();
    brief
        .post(
            &post(vec![
                item("next", "Next action", "Reply to Sanne"),
                item("cal:1", "Today", "Stand-up"),
                item("t", "Tasks", "Renew passport"),
            ]),
            1_000,
        )
        .unwrap();
    brief
        .post(&replace(&["Next action", "Today"], Vec::new()), 2_000)
        .unwrap();
    assert_eq!(brief.sections, ["Tasks"]);
    assert_eq!(brief.summary()["sections"], json!(["Tasks"]));
    // Coming back later puts a section at the end.
    brief
        .post(&post(vec![item("cal:2", "Today", "Review")]), 3_000)
        .unwrap();
    assert_eq!(brief.sections, ["Tasks", "Today"]);
    brief
        .post(
            &BriefPostParams {
                retire_ids: vec!["cal:2".to_owned()],
                ..BriefPostParams::default()
            },
            4_000,
        )
        .unwrap();
    assert_eq!(brief.sections, ["Tasks"]);

    // A brief saved with empty sections drops them when it loads.
    let saved = json!({"items":[], "sections":["Next action","Today"], "retired":[]});
    assert!(Brief::from_value_lossy(saved).sections.is_empty());
}

fn session(id: Option<&str>, conversation: Option<&str>) -> BriefSession {
    BriefSession {
        id: id.map(str::to_owned),
        provider: conversation.map(|_| AgentProvider::Claude),
        conversation_id: conversation.map(str::to_owned),
    }
}

#[test]
fn a_session_link_finds_the_row_or_offers_a_restore() {
    let rows = [
        ("claude-1", Some(AgentProvider::Claude), Some("conv-a")),
        ("codex-1", Some(AgentProvider::Codex), Some("conv-b")),
        ("shell-1", None, None),
    ];
    // By agtk session id first.
    assert_eq!(
        session(Some("claude-1"), Some("conv-x")).target(rows),
        SessionTarget::Select("claude-1".to_owned())
    );
    // The id is gone, but the conversation was restored under a new row.
    assert_eq!(
        session(Some("claude-9"), Some("conv-a")).target(rows),
        SessionTarget::Select("claude-1".to_owned())
    );
    // A conversation is only matched for the same provider.
    let codex_conversation_as_claude = session(None, Some("conv-b"));
    assert_eq!(
        codex_conversation_as_claude.target(rows),
        SessionTarget::Restore {
            provider: AgentProvider::Claude,
            conversation_id: "conv-b".to_owned()
        }
    );
    assert_eq!(
        session(Some("claude-9"), None).target(rows),
        SessionTarget::Gone
    );
}

#[test]
fn session_links_are_validated_stored_and_returned() {
    let mut brief = Brief::default();
    let mut linked = item("agent:review", "Waiting on you", "Review agent is waiting");
    linked.session = Some(session(Some("claude-1"), Some("conv-a")));
    brief.post(&post(vec![linked.clone()]), 1_000).unwrap();
    assert_eq!(
        brief.summary()["items"][0]["session"],
        json!({"id":"claude-1","provider":"claude","conversationId":"conv-a"})
    );

    // A new session id after a restore counts as an update.
    linked.session = Some(session(Some("claude-2"), Some("conv-a")));
    let changes = brief.post(&post(vec![linked]), 2_000).unwrap();
    assert_eq!(changes.updated, ["agent:review"]);

    for bad in [
        BriefSession {
            id: None,
            provider: None,
            conversation_id: None,
        },
        BriefSession {
            id: None,
            provider: None,
            conversation_id: Some("conv-a".to_owned()),
        },
        session(Some("has space"), None),
    ] {
        let mut invalid = item("x", "Today", "X");
        invalid.session = Some(bad.clone());
        assert!(brief.post(&post(vec![invalid]), 3_000).is_err(), "{bad:?}");
    }
    let mut id_only = item("y", "Today", "Y");
    id_only.session = Some(session(Some("shell-1"), None));
    assert!(brief.post(&post(vec![id_only]), 3_000).is_ok());
}

#[test]
fn the_open_button_names_the_links_domain() {
    let domain = |link: Option<&str>| {
        let mut posted = item("a", "Today", "A");
        posted.link = link.map(str::to_owned);
        let mut brief = Brief::default();
        brief.post(&post(vec![posted]), 1_000).unwrap();
        brief.items[0].link_domain()
    };
    assert_eq!(
        domain(Some(
            "https://dev.azure.com/org/project/_git/repo/pullrequest/1"
        ))
        .as_deref(),
        Some("dev.azure.com")
    );
    assert_eq!(
        domain(Some("https://mail.google.com/mail/u/0/#inbox/abc")).as_deref(),
        Some("mail.google.com")
    );
    // User info before an @ is not the host.
    assert_eq!(
        domain(Some("https://dev.azure.com@evil.example/path")).as_deref(),
        Some("evil.example")
    );
    assert_eq!(
        domain(Some("http://localhost:8080/x")).as_deref(),
        Some("localhost")
    );
    assert_eq!(domain(None), None);
}

#[test]
fn header_links_are_kept_until_a_post_replaces_them() {
    let calendar = BriefLink {
        label: "Google Calendar".to_owned(),
        url: "https://calendar.google.com/calendar/u/0/r".to_owned(),
    };
    let todoist = BriefLink {
        label: "Todoist".to_owned(),
        url: "https://app.todoist.com/app/today".to_owned(),
    };
    let mut brief = Brief::default();
    brief
        .post(
            &BriefPostParams {
                links: Some(vec![calendar.clone(), todoist.clone()]),
                ..BriefPostParams::default()
            },
            1_000,
        )
        .unwrap();
    assert_eq!(brief.links, [calendar.clone(), todoist.clone()]);
    assert_eq!(calendar.domain().as_deref(), Some("calendar.google.com"));
    assert_eq!(
        brief.summary()["links"],
        json!([
            {"label":"Google Calendar","url":"https://calendar.google.com/calendar/u/0/r"},
            {"label":"Todoist","url":"https://app.todoist.com/app/today"}
        ])
    );

    // A post without links leaves them; one with links replaces them all.
    brief
        .post(&post(vec![item("a", "Today", "A")]), 2_000)
        .unwrap();
    assert_eq!(brief.links.len(), 2);
    brief
        .post(
            &BriefPostParams {
                links: Some(vec![todoist.clone()]),
                ..BriefPostParams::default()
            },
            3_000,
        )
        .unwrap();
    assert_eq!(brief.links, std::slice::from_ref(&todoist));

    let link = |label: &str, url: &str| BriefLink {
        label: label.to_owned(),
        url: url.to_owned(),
    };
    for links in [
        vec![link("Files", "file:///home")],
        vec![link(" Calendar", "https://calendar.google.com/")],
        vec![link(&"x".repeat(31), "https://calendar.google.com/")],
        vec![todoist.clone(); 9],
    ] {
        let invalid = BriefPostParams {
            links: Some(links),
            ..BriefPostParams::default()
        };
        assert!(brief.post(&invalid, 4_000).is_err());
        assert_eq!(brief.links, std::slice::from_ref(&todoist));
    }

    brief
        .post(
            &BriefPostParams {
                links: Some(Vec::new()),
                ..BriefPostParams::default()
            },
            5_000,
        )
        .unwrap();
    assert!(brief.links.is_empty());
}

fn clear(id: &str, commented_at: u64) -> BriefPostParams {
    BriefPostParams {
        clear_comments: vec![BriefCommentClear {
            id: id.to_owned(),
            commented_at,
        }],
        ..BriefPostParams::default()
    }
}

fn retire(id: &str) -> BriefPostParams {
    BriefPostParams {
        retire_ids: vec![id.to_owned()],
        ..BriefPostParams::default()
    }
}

fn commented(id: &str) -> Brief {
    let mut brief = Brief::default();
    brief
        .post(&post(vec![item(id, "Today", "A")]), 1_000)
        .unwrap();
    assert!(brief.set_comment(id, "won't do", 2_000).unwrap());
    brief
}

#[test]
fn comments_are_written_edited_and_removed_in_the_panel() {
    let mut brief = commented("a");
    assert_eq!(get(&brief, "a").comment.as_deref(), Some("won't do"));
    assert_eq!(get(&brief, "a").commented_at, Some(2_000));
    assert!(!get(&brief, "a").read, "a comment sets no other mark");

    // The same words change nothing, so a clear that read them still applies.
    assert!(!brief.set_comment("a", "  won't do\n", 3_000).unwrap());
    assert_eq!(get(&brief, "a").commented_at, Some(2_000));
    assert!(brief.set_comment("a", "doing\nthe review", 4_000).unwrap());
    assert_eq!(get(&brief, "a").commented_at, Some(4_000));
    // Pasted control characters do not reach the assistant.
    assert!(brief.set_comment("a", "x\x1b[2Jy\r", 5_000).unwrap());
    assert_eq!(get(&brief, "a").comment.as_deref(), Some("x[2Jy"));

    assert!(brief.set_comment("a", "  \n ", 6_000).unwrap());
    assert_eq!(get(&brief, "a").comment, None);
    assert_eq!(get(&brief, "a").commented_at, None);
    assert!(!brief.set_comment("a", "", 7_000).unwrap());
    assert!(!brief.set_comment("missing", "hello", 7_000).unwrap());
}

#[test]
fn too_long_a_comment_is_refused_and_changes_nothing() {
    let mut brief = commented("a");
    let longest = "x".repeat(agtk::brief::MAX_COMMENT_CHARS);
    assert!(
        brief
            .set_comment("a", &format!("{longest}y"), 3_000)
            .is_err()
    );
    assert_eq!(get(&brief, "a").comment.as_deref(), Some("won't do"));
    assert!(brief.set_comment("a", &longest, 3_000).unwrap());
}

#[test]
fn get_brief_reports_comments_and_saved_briefs_without_them_load() {
    let brief = commented("a");
    let summary = brief.summary();
    assert_eq!(summary["items"][0]["comment"], "won't do");
    assert_eq!(summary["items"][0]["commentedAt"], 2_000);
    let mut saved = serde_json::to_value(&brief).unwrap();
    let item = saved["items"][0].as_object_mut().unwrap();
    item.remove("comment");
    item.remove("commentedAt");
    assert_eq!(Brief::from_value_lossy(saved).items[0].comment, None);
}

#[test]
fn a_repost_keeps_the_comment_and_done_leaves_it_alone() {
    let mut brief = commented("a");
    brief
        .post(&post(vec![item("a", "Tomorrow", "Changed")]), 3_000)
        .unwrap();
    assert_eq!(get(&brief, "a").comment.as_deref(), Some("won't do"));
    assert_eq!(get(&brief, "a").commented_at, Some(2_000));
    brief
        .post(
            &replace(&["Tomorrow"], vec![item("a", "Tomorrow", "Again")]),
            4_000,
        )
        .unwrap();
    assert_eq!(get(&brief, "a").comment.as_deref(), Some("won't do"));

    assert!(brief.toggle_done("a", 5_000));
    assert_eq!(get(&brief, "a").comment.as_deref(), Some("won't do"));
    assert_eq!(brief.summary()["items"][0]["comment"], "won't do");
}

#[test]
fn clearing_a_comment_needs_the_one_the_assistant_read() {
    let mut brief = commented("a");
    // The user edited it after the assistant read it at 2_000.
    brief.set_comment("a", "actually, do it", 3_000).unwrap();
    let changes = brief.post(&clear("a", 2_000), 4_000).unwrap();
    assert!(changes.comments_cleared.is_empty());
    assert_eq!(get(&brief, "a").comment.as_deref(), Some("actually, do it"));

    let changes = brief.post(&clear("a", 3_000), 5_000).unwrap();
    assert_eq!(changes.comments_cleared, ["a"]);
    assert_eq!(get(&brief, "a").comment, None);
    assert_eq!(get(&brief, "a").commented_at, None);
    // Clearing again, or something unknown, is not an error.
    assert!(
        brief
            .post(&clear("a", 3_000), 6_000)
            .unwrap()
            .comments_cleared
            .is_empty()
    );
    assert!(
        brief
            .post(&clear("gone", 3_000), 6_000)
            .unwrap()
            .comments_cleared
            .is_empty()
    );
}

#[test]
fn a_retired_item_keeps_its_pending_comment_and_a_repost_restores_it() {
    for params in [retire("a"), replace(&["Today"], Vec::new())] {
        let mut brief = commented("a");
        let changes = brief.post(&params, 3_000).unwrap();
        assert_eq!(changes.retired, ["a"]);
        assert!(brief.items.is_empty());
        let summary = brief.summary();
        assert_eq!(summary["retired"][0]["id"], "a");
        assert_eq!(summary["retired"][0]["comment"], "won't do");
        assert_eq!(summary["retired"][0]["commentedAt"], 2_000);

        let changes = brief
            .post(&post(vec![item("a", "Today", "A")]), 4_000)
            .unwrap();
        assert_eq!(changes.added, ["a"]);
        assert_eq!(get(&brief, "a").comment.as_deref(), Some("won't do"));
        assert_eq!(get(&brief, "a").commented_at, Some(2_000));
        assert!(brief.retired.is_empty());
    }
}

#[test]
fn clearing_a_retired_comment_forgets_the_entry_only_when_nothing_else_is_kept() {
    let mut brief = commented("a");
    brief.post(&retire("a"), 3_000).unwrap();
    assert!(
        brief
            .post(&clear("a", 1), 4_000)
            .unwrap()
            .comments_cleared
            .is_empty()
    );
    assert_eq!(brief.retired.len(), 1);
    assert_eq!(
        brief
            .post(&clear("a", 2_000), 4_000)
            .unwrap()
            .comments_cleared,
        ["a"]
    );
    assert!(brief.retired.is_empty());

    let mut brief = commented("a");
    brief.hide("a");
    brief.post(&retire("a"), 3_000).unwrap();
    brief.post(&clear("a", 2_000), 4_000).unwrap();
    assert_eq!(brief.retired.len(), 1);
    assert!(brief.retired[0].hidden);
    assert_eq!(brief.retired[0].comment, None);
}

#[test]
fn one_post_can_retire_an_item_and_clear_its_comment() {
    let mut brief = commented("a");
    let mut params = retire("a");
    params.clear_comments = clear("a", 2_000).clear_comments;
    let changes = brief.post(&params, 3_000).unwrap();
    assert_eq!(changes.comments_cleared, ["a"]);
    assert!(brief.retired.is_empty());
}

#[test]
fn a_pending_comment_outlasts_the_thirty_days_and_the_cap_and_unhide() {
    let mut brief = commented("a");
    brief.hide("a");
    brief.post(&retire("a"), 3_000).unwrap();
    // Unhiding leaves no mark, but the comment keeps the entry.
    assert!(brief.unhide("a"));
    assert_eq!(brief.retired[0].comment.as_deref(), Some("won't do"));

    brief
        .post(&post(Vec::new()), 3_000 + REMEMBER_MILLIS + 1)
        .unwrap();
    assert_eq!(brief.retired[0].comment.as_deref(), Some("won't do"));

    // The cap forgets marks, oldest first, before any comment. Items are
    // retired 200 at a time, because a post holds no more than that.
    let now = 3_000 + REMEMBER_MILLIS + 2;
    for round in 0..11 {
        let items = (0..200)
            .map(|index| item(&format!("r{round}-{index}"), "Today", "R"))
            .collect::<Vec<_>>();
        brief.post(&post(items), now).unwrap();
        for index in 0..200 {
            brief.hide(&format!("r{round}-{index}"));
        }
        brief.post(&replace(&["Today"], Vec::new()), now).unwrap();
    }
    assert_eq!(brief.retired.len(), 2000);
    assert_eq!(brief.retired[0].id, "a");

    // Once cleared, its age counts again.
    assert_eq!(
        brief
            .post(&clear("a", 2_000), now)
            .unwrap()
            .comments_cleared,
        ["a"]
    );
    assert!(brief.retired.iter().all(|retired| retired.id != "a"));
}

#[test]
fn the_tab_title_counts_from_the_last_post_even_one_that_changes_nothing() {
    const MINUTE: u64 = 60_000;
    let mut brief = Brief::default();
    assert_eq!(brief.tab_title(MINUTE), "Brief");
    brief
        .post(&post(vec![item("a", "Today", "A")]), MINUTE)
        .unwrap();
    assert_eq!(brief.summary()["lastPostedAt"], MINUTE);
    for (age, title) in [
        (0, "Brief (now)"),
        (MINUTE - 1, "Brief (now)"),
        (MINUTE, "Brief (1m)"),
        (2 * MINUTE, "Brief (2m)"),
        (59 * MINUTE, "Brief (59m)"),
        (60 * MINUTE, "Brief (1h)"),
        (24 * 60 * MINUTE - 1, "Brief (23h)"),
        (24 * 60 * MINUTE, "Brief (1d)"),
    ] {
        assert_eq!(brief.tab_title(MINUTE + age), title);
    }
    // A clock that went back is not a negative age.
    assert_eq!(brief.tab_title(0), "Brief (now)");

    brief.post(&post(Vec::new()), 10 * MINUTE).unwrap();
    assert_eq!(get(&brief, "a").updated_at, MINUTE);
    assert_eq!(brief.tab_title(12 * MINUTE), "Brief (2m)");

    // A refused post is no update.
    let refused = post(vec![item("", "Today", "A")]);
    assert!(brief.post(&refused, 11 * MINUTE).is_err());
    assert_eq!(brief.tab_title(12 * MINUTE), "Brief (2m)");

    // A brief saved before posts were timed goes by its newest item.
    let mut saved = serde_json::to_value(&brief).unwrap();
    saved.as_object_mut().unwrap().remove("lastPostedAt");
    assert_eq!(
        Brief::from_value_lossy(saved).tab_title(3 * MINUTE),
        "Brief (2m)"
    );
}
