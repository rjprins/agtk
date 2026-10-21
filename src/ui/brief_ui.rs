//! The Brief page of the right sidebar: the daily brief an assistant posts over MCP.

use super::*;
use crate::brief::{Brief, BriefItem, BriefPostParams, PREFERENCE_KEY};
use crate::persist::now_millis;

pub(super) const BRIEF_PAGE: &str = "brief";
const EMPTY_TEXT: &str = "No brief yet. Your assistant posts it with the post_brief MCP tool.";

#[derive(Clone)]
pub(super) struct BriefPage {
    pub root: gtk::Box,
    pub mark_all_read: gtk::Button,
    pub rows: gtk::Box,
}

impl BriefPage {
    pub fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        toolbar.set_margin_start(12);
        toolbar.set_margin_end(12);
        toolbar.set_margin_bottom(4);
        let hint = gtk::Label::new(Some("Removing only hides an item."));
        hint.set_xalign(0.0);
        hint.set_hexpand(true);
        hint.set_wrap(true);
        hint.add_css_class("caption");
        hint.add_css_class("dim-label");
        let mark_all_read = gtk::Button::builder()
            .label("Mark all read")
            .action_name("win.brief-read-all")
            .sensitive(false)
            .build();
        mark_all_read.add_css_class("flat");
        toolbar.append(&hint);
        toolbar.append(&mark_all_read);
        root.append(&toolbar);

        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();
        let rows = gtk::Box::new(gtk::Orientation::Vertical, 2);
        rows.set_margin_start(6);
        rows.set_margin_end(6);
        rows.set_margin_bottom(8);
        scroll.set_child(Some(&rows));
        root.append(&scroll);
        Self {
            root,
            mark_all_read,
            rows,
        }
    }
}

impl Workspace {
    pub(super) fn install_brief_actions(&self) {
        self.add_target_action("brief-read", |workspace, id| {
            workspace.change_brief(|brief| brief.mark_read(id));
        });
        self.add_target_action("brief-remove", |workspace, id| {
            workspace.change_brief(|brief| brief.remove(id));
        });
        self.add_target_action("brief-open", |workspace, id| {
            let link = workspace
                .brief
                .borrow()
                .items
                .iter()
                .find(|item| item.id == id)
                .and_then(|item| item.link.clone());
            if let Some(link) = link {
                workspace.open_brief_link(&link);
                workspace.change_brief(|brief| brief.mark_read(id));
            }
        });
        let action = gio::SimpleAction::new("brief-read-all", None);
        let workspace = self.clone();
        action.connect_activate(move |_, _| workspace.change_brief(Brief::mark_all_read));
        self.window.add_action(&action);
    }

    pub(super) fn load_brief(&self, brief: Brief) {
        *self.brief.borrow_mut() = brief;
        self.render_brief();
    }

    pub(super) fn handle_brief_control(&self, pending: PendingRequest) {
        let ControlCommand::BriefPost(params) = pending.request.command.clone() else {
            unreachable!("caller filters brief commands");
        };
        self.post_brief(&params, pending);
    }

    fn post_brief(&self, params: &BriefPostParams, pending: PendingRequest) {
        let Some(store) = self.store.borrow().clone() else {
            self.report_failure(
                Some(pending),
                ErrorCode::InternalError,
                "Workspace is loading",
                "the brief is not available yet".to_owned(),
            );
            return;
        };
        let changes = match self.brief.borrow_mut().post(params, now_millis()) {
            Ok(changes) => changes,
            Err(error) => {
                self.report_failure(
                    Some(pending),
                    ErrorCode::InvalidParams,
                    "Brief items are invalid",
                    error.to_string(),
                );
                return;
            }
        };
        self.render_brief();
        // Applied before saving, so a post that arrives during the save builds on this one.
        let value = serde_json::to_value(&*self.brief.borrow()).unwrap_or_default();
        self.run_io(
            move || store.set_preference(PREFERENCE_KEY, &value),
            move |workspace, result| {
                let id = pending.request.id.clone();
                match result {
                    Ok(()) => {
                        let mut summary = workspace.brief.borrow().summary();
                        summary["changes"] = serde_json::json!(changes);
                        let _ = pending.respond(ControlResponse::success(id, summary));
                    }
                    Err(error) => workspace.report_launch_failure(
                        Some(pending),
                        "Could not save the brief",
                        error.to_string(),
                    ),
                }
            },
        );
    }

    fn change_brief(&self, change: impl FnOnce(&mut Brief) -> bool) {
        if !change(&mut self.brief.borrow_mut()) {
            return;
        }
        self.render_brief();
        let value = serde_json::to_value(&*self.brief.borrow()).unwrap_or_default();
        self.save_preference(PREFERENCE_KEY, value);
    }

    fn open_brief_link(&self, url: &str) {
        let workspace = self.clone();
        gtk::UriLauncher::new(url).launch(
            Some(&self.window),
            None::<&gio::Cancellable>,
            move |result| {
                if let Err(error) = result {
                    workspace.show_error(&format!("Could not open brief link: {error}"));
                }
            },
        );
    }

    pub(super) fn render_brief(&self) {
        let page = &self.changes.brief;
        while let Some(child) = page.rows.first_child() {
            page.rows.remove(&child);
        }
        let brief = self.brief.borrow();
        let unread = brief.unread_count();
        page.mark_all_read.set_sensitive(unread > 0);
        let title = if unread > 0 {
            format!("Brief ({unread})")
        } else {
            "Brief".to_owned()
        };
        self.changes.pages.page(&page.root).set_title(Some(&title));
        let sections = brief.visible_sections();
        if sections.is_empty() {
            let message = gtk::Label::new(Some(EMPTY_TEXT));
            message.set_wrap(true);
            message.set_margin_top(24);
            message.set_margin_start(12);
            message.set_margin_end(12);
            message.add_css_class("dim-label");
            page.rows.append(&message);
            return;
        }
        for (section, items) in sections {
            let heading = gtk::Label::new(Some(section));
            heading.set_xalign(0.0);
            heading.set_margin_top(10);
            heading.set_margin_start(6);
            heading.set_margin_bottom(2);
            heading.add_css_class("caption-heading");
            heading.add_css_class("dim-label");
            page.rows.append(&heading);
            for item in items {
                page.rows.append(&brief_row(item));
            }
        }
    }
}

fn brief_row(item: &BriefItem) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    row.set_widget_name(&item.id);
    row.set_margin_start(6);
    let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    dot.set_valign(gtk::Align::Start);
    dot.set_margin_top(7);
    dot.set_margin_end(4);
    dot.add_css_class("brief-unread-dot");
    dot.set_opacity(if item.read { 0.0 } else { 1.0 });
    dot.set_tooltip_text((!item.read).then_some("New"));
    row.append(&dot);
    let text = gtk::Label::new(Some(&item.text));
    text.set_xalign(0.0);
    text.set_hexpand(true);
    text.set_wrap(true);
    text.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    text.set_selectable(true);
    if !item.read {
        text.add_css_class("heading");
    }
    row.append(&text);
    let target = item.id.to_variant();
    if item.link.is_some() {
        let open = brief_button("web-browser-symbolic", "Open the link and mark it read");
        open.set_action_name(Some("win.brief-open"));
        open.set_action_target_value(Some(&target));
        row.append(&open);
    }
    if !item.read {
        let read = brief_button("object-select-symbolic", "Mark read");
        read.set_action_name(Some("win.brief-read"));
        read.set_action_target_value(Some(&target));
        row.append(&read);
    }
    let remove = brief_button(
        "window-close-symbolic",
        "Remove from the brief. The task or mail itself is not changed.",
    );
    remove.set_action_name(Some("win.brief-remove"));
    remove.set_action_target_value(Some(&target));
    row.append(&remove);
    row
}

fn brief_button(icon: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .valign(gtk::Align::Start)
        .build();
    button.add_css_class("flat");
    button
}
