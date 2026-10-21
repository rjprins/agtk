//! The Brief page of the right sidebar: the daily brief an assistant posts over MCP.

use super::*;
use crate::brief::{
    Brief, BriefItem, BriefPostParams, BriefSession, PREFERENCE_KEY, SessionTarget,
};
use crate::persist::now_millis;
use crate::providers::AgentProvider;

pub(super) const BRIEF_PAGE: &str = "brief";
pub(super) const HIDDEN_EXPANDED_PREFERENCE: &str = "briefHiddenExpanded";
const EMPTY_TEXT: &str = "No brief yet. Your assistant posts it with the post_brief MCP tool.";

#[derive(Clone)]
pub(super) struct BriefPage {
    pub root: gtk::Box,
    pub mark_all_read: gtk::Button,
    pub rows: gtk::Box,
    /// Hidden items, kept at the bottom like Inactive Projects in the sidebar.
    pub hidden: gtk::Box,
}

impl BriefPage {
    pub fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        toolbar.set_margin_start(12);
        toolbar.set_margin_end(12);
        toolbar.set_margin_bottom(4);
        let hint = gtk::Label::new(Some("Click an item to mark it read or unread."));
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
        let hidden = gtk::Box::new(gtk::Orientation::Vertical, 2);
        hidden.set_margin_start(6);
        hidden.set_margin_end(6);
        hidden.set_margin_bottom(8);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&rows);
        let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        spacer.set_vexpand(true);
        content.append(&spacer);
        content.append(&hidden);
        scroll.set_child(Some(&content));
        root.append(&scroll);
        Self {
            root,
            mark_all_read,
            rows,
            hidden,
        }
    }
}

impl Workspace {
    pub(super) fn install_brief_actions(&self) {
        self.add_target_action("brief-toggle-read", |workspace, id| {
            workspace.change_brief(|brief| brief.toggle_read(id));
        });
        self.add_target_action("brief-done", |workspace, id| {
            workspace.change_brief(|brief| brief.toggle_done(id, now_millis()));
        });
        self.add_target_action("brief-hide", |workspace, id| {
            workspace.change_brief(|brief| brief.hide(id));
        });
        self.add_target_action("brief-unhide", |workspace, id| {
            workspace.change_brief(|brief| brief.unhide(id));
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
        self.add_target_action("brief-session", |workspace, id| {
            let session = workspace
                .brief
                .borrow()
                .items
                .iter()
                .find(|item| item.id == id)
                .and_then(|item| item.session.clone());
            if let Some(session) = session {
                workspace.change_brief(|brief| brief.mark_read(id));
                workspace.open_brief_session(&session);
            }
        });
        let action = gio::SimpleAction::new("brief-read-all", None);
        let workspace = self.clone();
        action.connect_activate(move |_, _| workspace.change_brief(Brief::mark_all_read));
        self.window.add_action(&action);
        let action = gio::SimpleAction::new("brief-toggle-hidden", None);
        let workspace = self.clone();
        action.connect_activate(move |_, _| {
            let expanded = !workspace.brief_hidden_expanded.get();
            workspace.brief_hidden_expanded.set(expanded);
            workspace.save_preference(HIDDEN_EXPANDED_PREFERENCE, serde_json::json!(expanded));
            workspace.render_brief();
        });
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

    /// Selects the item's session, or offers to restore its closed conversation.
    fn open_brief_session(&self, session: &BriefSession) {
        let target = {
            let sessions = self.sessions.borrow();
            session.target(sessions.iter().map(|(id, live)| {
                (
                    id.as_str(),
                    AgentProvider::from_kind(live.record.kind),
                    live.record.conversation_id.as_deref(),
                )
            }))
        };
        match target {
            SessionTarget::Select(id) => {
                let row = self.sessions.borrow().get(&id).map(|live| live.row.clone());
                if let Some(row) = row {
                    self.list.select_row(Some(&row));
                }
            }
            SessionTarget::Restore {
                provider,
                conversation_id,
            } => {
                let dialog = adw::AlertDialog::new(
                    Some("Restore the Agent Session?"),
                    Some("This session is closed. Restore its conversation where it last ran?"),
                );
                dialog.add_responses(&[("cancel", "_Cancel"), ("restore", "_Restore")]);
                dialog.set_response_appearance("restore", adw::ResponseAppearance::Suggested);
                dialog.set_default_response(Some("restore"));
                dialog.set_close_response("cancel");
                let workspace = self.clone();
                dialog.connect_response(Some("restore"), move |_, _| {
                    workspace.restore_conversation(provider, conversation_id.clone());
                });
                dialog.present(Some(&self.window));
            }
            SessionTarget::Gone => self.show_error(
                "This agent session is closed, and agtk does not know its conversation",
            ),
        }
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
        for rows in [&page.rows, &page.hidden] {
            while let Some(child) = rows.first_child() {
                rows.remove(&child);
            }
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
        let hidden = brief.hidden_items();
        if sections.is_empty() && hidden.is_empty() {
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
        if hidden.is_empty() {
            return;
        }
        let expanded = self.brief_hidden_expanded.get();
        page.hidden.append(&hidden_header(hidden.len(), expanded));
        if expanded {
            for item in hidden {
                page.hidden.append(&hidden_row(item));
            }
        }
    }
}

/// Heads the hidden items at the bottom; the list under it starts collapsed.
fn hidden_header(count: usize, expanded: bool) -> gtk::Button {
    let contents = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    contents.add_css_class("dim-label");
    let icon = gtk::Image::from_icon_name(if expanded {
        "pan-down-symbolic"
    } else {
        "pan-end-symbolic"
    });
    icon.set_pixel_size(14);
    let label = gtk::Label::new(Some("Hidden"));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label.add_css_class("caption-heading");
    let count_label = gtk::Label::new(Some(&count.to_string()));
    count_label.add_css_class("numeric");
    contents.append(&icon);
    contents.append(&label);
    contents.append(&count_label);
    let toggle = gtk::Button::builder()
        .child(&contents)
        .action_name("win.brief-toggle-hidden")
        .tooltip_text(if expanded {
            "Collapse hidden items"
        } else {
            "Show hidden items"
        })
        .build();
    toggle.set_margin_top(10);
    toggle.add_css_class("flat");
    toggle.update_state(&[gtk::accessible::State::Expanded(Some(expanded))]);
    toggle
}

fn hidden_row(item: &BriefItem) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    row.set_widget_name(&item.id);
    row.set_margin_start(18);
    let text = gtk::Label::new(Some(&item.text));
    text.set_xalign(0.0);
    text.set_hexpand(true);
    text.set_wrap(true);
    text.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    text.add_css_class("dim-label");
    text.set_tooltip_text(Some(&item.section));
    if item.done {
        strike_through(&text);
    }
    row.append(&text);
    // Undoing done brings a done item back, so it needs no unhide button.
    if item.done {
        row.append(&done_button(item));
    } else {
        let unhide = brief_button("view-reveal-symbolic", "Unhide");
        unhide.set_action_name(Some("win.brief-unhide"));
        unhide.set_action_target_value(Some(&item.id.to_variant()));
        row.append(&unhide);
    }
    row
}

fn strike_through(text: &gtk::Label) {
    let strikethrough = gtk::pango::AttrList::new();
    strikethrough.insert(gtk::pango::AttrInt::new_strikethrough(true));
    text.set_attributes(Some(&strikethrough));
    text.add_css_class("dim-label");
}

fn done_button(item: &BriefItem) -> gtk::Button {
    let done = if item.done {
        let button = brief_button("object-select-symbolic", "Done. Click to undo.");
        button.add_css_class("success");
        button
    } else {
        brief_button(
            "object-select-symbolic",
            "Mark done and hide it. Your assistant completes the task.",
        )
    };
    done.set_action_name(Some("win.brief-done"));
    done.set_action_target_value(Some(&item.id.to_variant()));
    done
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
    if !item.read {
        text.add_css_class("heading");
    }
    // Done items are hidden, unless the assistant shows one again.
    if item.done {
        strike_through(&text);
    }
    row.append(&text);
    let target = item.id.to_variant();
    row.set_tooltip_text(Some(if item.read {
        "Click to mark unread"
    } else {
        "Click to mark read"
    }));
    row.set_cursor_from_name(Some("pointer"));
    // Buttons claim their own clicks, so only the rest of the row toggles read.
    let click = gtk::GestureClick::new();
    let click_target = target.clone();
    click.connect_released(move |gesture, _, _, _| {
        if let Some(row) = gesture.widget() {
            let _ = row.activate_action("win.brief-toggle-read", Some(&click_target));
        }
    });
    row.add_controller(click);
    if item.session.is_some() {
        let open = brief_button(
            "utilities-terminal-symbolic",
            "Go to the agent session and mark it read",
        );
        open.set_action_name(Some("win.brief-session"));
        open.set_action_target_value(Some(&target));
        row.append(&open);
    }
    if item.link.is_some() {
        let tooltip = match item.link_domain() {
            Some(domain) => format!("Open {domain}"),
            None => "Open the link".to_owned(),
        };
        let open = brief_button("web-browser-symbolic", &tooltip);
        open.set_action_name(Some("win.brief-open"));
        open.set_action_target_value(Some(&target));
        row.append(&open);
    }
    row.append(&done_button(item));
    let hide = brief_button(
        "view-conceal-symbolic",
        "Hide. The task or mail itself is not changed.",
    );
    hide.set_action_name(Some("win.brief-hide"));
    hide.set_action_target_value(Some(&target));
    row.append(&hide);
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
