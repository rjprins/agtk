//! The Updates page of the right sidebar: comments and votes by other people on
//! the PRs the user wrote or reviews, newest first, with a dot on the tab for
//! what came in since the user last looked.

use super::*;
use crate::azure::{PrUpdate, PrUpdateKind, pr_updates};

pub(super) const UPDATES_PAGE: &str = "updates";
/// Enough to scroll back a few days on a small team.
const MAX_UPDATES: usize = 150;
/// How long a closed update stays closed. Long after its PR is done.
const REMEMBER_CLOSED_MILLIS: u64 = 90 * 24 * 60 * 60 * 1000;
const EMPTY_TEXT: &str = "No PR updates yet. Comments and votes by others on PRs you wrote or review show up here once agtk has checked the projects in the sidebar.";

#[derive(Clone)]
pub(super) struct UpdatesPage {
    pub root: gtk::Box,
    pub rows: gtk::Box,
    /// Closed updates, kept at the bottom like Hidden in the brief.
    pub closed: gtk::Box,
}

impl UpdatesPage {
    pub fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();
        let rows = gtk::Box::new(gtk::Orientation::Vertical, 2);
        rows.set_margin_start(6);
        rows.set_margin_end(6);
        rows.set_margin_bottom(8);
        let closed = gtk::Box::new(gtk::Orientation::Vertical, 2);
        closed.set_margin_start(6);
        closed.set_margin_end(6);
        closed.set_margin_bottom(8);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&rows);
        let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        spacer.set_vexpand(true);
        content.append(&spacer);
        content.append(&closed);
        scroll.set_child(Some(&content));
        root.append(&scroll);
        Self { root, rows, closed }
    }
}

impl Workspace {
    /// The page counts as seen only while the user can see it, so follow the
    /// sidebar and the window's focus too.
    pub(super) fn install_pr_updates(&self) {
        let workspace = self.clone();
        self.changes
            .split
            .connect_show_sidebar_notify(move |_| workspace.sync_pr_updates_view());
        let workspace = self.clone();
        self.window
            .connect_is_active_notify(move |_| workspace.sync_pr_updates_view());
        self.render_pr_updates();
    }

    /// Whether the user can see the Updates page right now.
    fn viewing_pr_updates(&self) -> bool {
        self.window.is_active()
            && self.changes.split.shows_sidebar()
            && self.changes.pages.visible_child_name().as_deref() == Some(UPDATES_PAGE)
    }

    /// Renders again when the user starts or stops looking at the page, which
    /// is what marks updates seen and clears the dot.
    pub(super) fn sync_pr_updates_view(&self) {
        if self.viewing_pr_updates() != self.pr_updates_seen_before.get().is_some() {
            self.render_pr_updates();
        }
    }

    /// Every project's updates, newest first.
    pub(super) fn pr_update_list(&self) -> Vec<PrUpdate> {
        let mut updates = self
            .pr_context_cache
            .borrow()
            .values()
            .flat_map(|context| {
                pr_updates(
                    context.pull_requests.iter().map(|item| &item.pull_request),
                    context.current_user.as_deref(),
                )
            })
            .collect::<Vec<_>>();
        updates.sort_by_key(|update| std::cmp::Reverse(update.at));
        updates.truncate(MAX_UPDATES);
        updates
    }

    /// Rebuilds the page and its tab's dot. While the user looks at the page,
    /// what it shows counts as seen; the rows that were new when they opened it
    /// stay marked until they leave it.
    pub(super) fn render_pr_updates(&self) {
        let (updates, closed): (Vec<_>, Vec<_>) = {
            let preferences = self.pr_preferences.borrow();
            self.pr_update_list()
                .into_iter()
                .partition(|update| !preferences.updates_closed.contains_key(&update.id))
        };
        let newest = updates.first().map_or(0, |update| update.at);
        let seen = self.pr_preferences.borrow().updates_seen_at;
        let closed_expanded = self.pr_preferences.borrow().updates_closed_expanded;
        let viewing = self.viewing_pr_updates();
        let new_since = if viewing {
            let before = self.pr_updates_seen_before.get().unwrap_or(seen);
            self.pr_updates_seen_before.set(Some(before));
            before
        } else {
            self.pr_updates_seen_before.set(None);
            seen
        };
        if viewing && newest > seen {
            self.save_pr_updates_seen(newest);
        }
        // The inline switcher draws no needs-attention dot, so the title carries it.
        let title = if !viewing && newest > seen {
            "Updates ●"
        } else {
            "Updates"
        };
        let page = self.changes.pages.page(&self.changes.updates.root);
        if page.title().as_deref() != Some(title) {
            page.set_title(Some(title));
        }

        let rows = &self.changes.updates.rows;
        for list in [rows, &self.changes.updates.closed] {
            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
        }
        if !closed.is_empty() {
            let header =
                brief_ui::collapsible_header("Closed", Some(closed.len()), 0, closed_expanded);
            header.set_tooltip_text(Some(if closed_expanded {
                "Collapse closed updates"
            } else {
                "Show closed updates"
            }));
            let workspace = self.clone();
            header.connect_clicked(move |_| {
                workspace.change_pr_preferences(|preferences| {
                    preferences.updates_closed_expanded = !preferences.updates_closed_expanded;
                });
                workspace.render_pr_updates();
            });
            self.changes.updates.closed.append(&header);
            if closed_expanded {
                for update in &closed {
                    self.changes
                        .updates
                        .closed
                        .append(&self.pr_update_row(update, false, true));
                }
            }
        }
        if updates.is_empty() {
            let message = gtk::Label::new(Some(if closed.is_empty() {
                EMPTY_TEXT
            } else {
                "No open PR updates. Closed ones are at the bottom."
            }));
            message.set_wrap(true);
            message.add_css_class("dim-label");
            message.set_margin_top(24);
            message.set_margin_start(12);
            message.set_margin_end(12);
            rows.append(&message);
            return;
        }
        for update in &updates {
            rows.append(&self.pr_update_row(update, update.at > new_since, false));
        }
    }

    fn save_pr_updates_seen(&self, at: u64) {
        self.change_pr_preferences(|preferences| preferences.updates_seen_at = at);
    }

    /// Closes an update, or opens a closed one again, and shows the change.
    fn set_pr_update_closed(&self, id: &str, closed: bool) {
        let now = crate::persist::now_millis();
        self.change_pr_preferences(|preferences| {
            if closed {
                preferences.updates_closed.insert(id.to_owned(), now);
            } else {
                preferences.updates_closed.remove(id);
            }
            preferences
                .updates_closed
                .retain(|_, at| now.saturating_sub(*at) < REMEMBER_CLOSED_MILLIS);
        });
        self.render_pr_updates();
    }

    fn change_pr_preferences(&self, change: impl FnOnce(&mut crate::azure::PrPreferences)) {
        let mut preferences = self.pr_preferences.borrow().clone();
        change(&mut preferences);
        *self.pr_preferences.borrow_mut() = preferences.clone();
        let Some(store) = self.store.borrow().clone() else {
            return;
        };
        let Ok(value) = serde_json::to_value(&preferences) else {
            return;
        };
        self.run_io(
            move || store.set_preference("pullRequests", &value),
            |workspace, result| {
                if let Err(error) = result {
                    workspace.show_error(&format!("Could not save PR updates: {error}"));
                }
            },
        );
    }

    /// The session working on this PR, if one is open.
    fn session_for_pr(&self, pr_id: u64) -> Option<String> {
        self.session_pr_cache
            .borrow()
            .iter()
            .filter(|(id, pull_request)| {
                pull_request.id == pr_id && self.sessions.borrow().contains_key(id.as_str())
            })
            .map(|(id, _)| id.clone())
            .min()
    }

    fn pr_update_row(&self, update: &PrUpdate, is_new: bool, closed: bool) -> gtk::Widget {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        row.add_css_class("pr-update");
        if closed {
            row.add_css_class("dim-label");
        }

        let body = gtk::Box::new(gtk::Orientation::Vertical, 2);
        body.set_hexpand(true);
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        if is_new {
            let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            dot.add_css_class("pr-update-dot");
            dot.set_valign(gtk::Align::Center);
            dot.set_tooltip_text(Some("New since you last looked"));
            top.append(&dot);
        }
        let (verb, verb_class) = match &update.kind {
            PrUpdateKind::Comment { .. } => ("commented", None),
            PrUpdateKind::Vote { vote } => match vote {
                10 => ("approved", Some("success")),
                5 => ("approved with suggestions", Some("success")),
                -5 => ("is waiting for the author", Some("warning")),
                -10 => ("rejected", Some("error")),
                _ => ("voted", None),
            },
        };
        let author = gtk::Label::new(Some(&update.author));
        author.add_css_class("heading");
        author.set_xalign(0.0);
        author.set_ellipsize(gtk::pango::EllipsizeMode::End);
        top.append(&author);
        let action = gtk::Label::new(Some(verb));
        action.set_xalign(0.0);
        action.set_ellipsize(gtk::pango::EllipsizeMode::End);
        if let Some(class) = verb_class {
            action.add_css_class(class);
        }
        top.append(&action);
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        top.append(&spacer);
        if update.at > 0 {
            let age = gtk::Label::new(Some(&status_ui::elapsed_since(update.at)));
            age.add_css_class("caption");
            age.add_css_class("dim-label");
            top.append(&age);
        }
        body.append(&top);

        let pr = gtk::Label::new(Some(&format!("#{} {}", update.pr_id, update.pr_title)));
        pr.add_css_class("caption");
        pr.add_css_class("dim-label");
        pr.set_xalign(0.0);
        pr.set_ellipsize(gtk::pango::EllipsizeMode::End);
        pr.set_tooltip_text(Some(&update.pr_title));
        body.append(&pr);

        if let PrUpdateKind::Comment {
            text,
            file_path,
            line,
        } = &update.kind
        {
            if let Some(path) = file_path {
                let location = match line {
                    Some(line) => format!("{path}:{line}"),
                    None => path.clone(),
                };
                let location = gtk::Label::new(Some(&location));
                location.add_css_class("caption");
                location.add_css_class("monospace");
                location.add_css_class("dim-label");
                location.set_xalign(0.0);
                location.set_ellipsize(gtk::pango::EllipsizeMode::Start);
                body.append(&location);
            }
            let text_label = gtk::Label::new(Some(text.trim()));
            text_label.set_xalign(0.0);
            text_label.set_wrap(true);
            text_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            text_label.set_lines(4);
            text_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            text_label.set_tooltip_text(Some(text.trim()));
            body.append(&text_label);
        }

        let open = gtk::Button::new();
        open.add_css_class("flat");
        open.add_css_class("pr-update-open");
        open.set_child(Some(&body));
        open.set_hexpand(true);
        open.set_tooltip_text(Some("Open in Azure DevOps"));
        let link = update.link();
        let workspace = self.clone();
        open.connect_clicked(move |_| workspace.open_pr_link(&link));
        row.append(&open);

        if let Some(session) = self.session_for_pr(update.pr_id) {
            let go = brief_ui::brief_button(
                "utilities-terminal-symbolic",
                "Go to the session on this PR and show its comments",
            );
            let workspace = self.clone();
            go.connect_clicked(move |_| {
                workspace.activate_session(&session);
                workspace.context_pr_comments_button.popup();
            });
            row.append(&go);
        }
        let (icon, tooltip) = if closed {
            ("view-reveal-symbolic", "Open again")
        } else {
            (
                "window-close-symbolic",
                "Close. The comment or vote itself is not changed.",
            )
        };
        let toggle = brief_ui::brief_button(icon, tooltip);
        let workspace = self.clone();
        let id = update.id.clone();
        toggle.connect_clicked(move |_| workspace.set_pr_update_closed(&id, !closed));
        row.append(&toggle);
        row.upcast()
    }
}
