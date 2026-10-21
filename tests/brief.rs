use agtk::brief::{Brief, BriefItemInput, BriefPostParams, REMEMBER_MILLIS};
use serde_json::json;

fn item(id: &str, section: &str, text: &str) -> BriefItemInput {
    BriefItemInput {
        id: id.to_owned(),
        section: section.to_owned(),
        text: text.to_owned(),
        link: None,
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
        retire_ids: Vec::new(),
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
    assert!(brief.items.iter().all(|item| !item.read && !item.removed));
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
    assert!(brief.remove("ado-pr:2"));

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
    assert!(get(&brief, "ado-pr:2").removed);
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
    // b was neither read nor removed, so nothing about it is remembered.
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
    brief.remove("todoist:1");
    brief.mark_read("gmail:abc");
    let changes = brief
        .post(
            &BriefPostParams {
                items: Vec::new(),
                replace_sections: vec!["Tasks".to_owned()],
                retire_ids: vec!["gmail:abc".to_owned(), "gmail:def".to_owned()],
            },
            2_000,
        )
        .unwrap();
    assert_eq!(changes.retired, ["todoist:1", "gmail:abc", "gmail:def"]);
    assert!(brief.items.is_empty());
    assert_eq!(
        serde_json::to_value(&brief.retired).unwrap(),
        json!([
            {"id":"todoist:1","read":false,"removed":true,"done":false,"doneAt":null,"retiredAt":2_000},
            {"id":"gmail:abc","read":true,"removed":false,"done":false,"doneAt":null,"retiredAt":2_000}
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
    assert!(get(&brief, "todoist:1").removed);
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
    assert!(!get(&brief, "todoist:1").removed);
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
    assert!(!brief.remove("missing"));
    assert!(brief.remove("b"));
    assert!(!brief.remove("b"));
    assert!(brief.mark_all_read());
    assert!(!brief.mark_all_read());
    assert!(!brief.mark_read("a"));
    // Removed items keep their own read mark.
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
fn done_toggles_with_its_time_and_also_marks_read() {
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
    assert!(done.done && done.read);
    assert_eq!(done.done_at, Some(2_000));
    assert_eq!(brief.unread_count(), 0);
    assert_eq!(brief.summary()["items"][0]["doneAt"], 2_000);

    // Undoing clears the mark and its time, but the item stays read.
    assert!(brief.toggle_done("todoist:7", 3_000));
    let undone = get(&brief, "todoist:7");
    assert!(!undone.done && undone.read);
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
            "id":"todoist:7","read":true,"removed":false,
            "done":true,"doneAt":2_000,"retiredAt":4_000
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
fn a_brief_saved_before_done_existed_still_loads() {
    let saved = json!({
        "items":[{"id":"a","section":"Today","text":"A","link":null,
                  "postedAt":1,"updatedAt":1,"read":true,"removed":false}],
        "sections":["Today"],
        "retired":[{"id":"b","read":false,"removed":true,"retiredAt":1}]
    });
    let brief = Brief::from_value_lossy(saved);
    assert_eq!(brief.items.len(), 1);
    assert!(brief.items[0].read && !brief.items[0].done);
    assert_eq!(brief.items[0].done_at, None);
    assert!(brief.retired[0].removed && !brief.retired[0].done);
}
