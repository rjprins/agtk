//! The Brief page of the right sidebar: the daily brief an assistant posts over MCP.

use super::*;
use crate::brief::{
    Brief, BriefItem, BriefPostParams, BriefSession, PREFERENCE_KEY, SessionTarget,
};
use crate::persist::now_millis;
use crate::providers::AgentProvider;

pub(super) const BRIEF_PAGE: &str = "brief";
pub(super) const HIDDEN_EXPANDED_PREFERENCE: &str = "briefHiddenExpanded";
/// The names of the sections the user collapsed. Assistants pick the names.
pub(super) const COLLAPSED_SECTIONS_PREFERENCE: &str = "briefCollapsedSections";
/// Keeps the time since the last post in the tab's title about right.
const BRIEF_TITLE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(20);
/// Lines a comment up under the item's text.
const COMMENT_MARGIN: i32 = 12;
const EMPTY_TEXT: &str = "No brief yet. Your assistant posts it with the post_brief MCP tool.";

/// An open comment box. It outlives the rows, which every change rebuilds, so
/// a post that arrives while the user types does not take the words away.
pub(super) struct CommentDraft {
    pub text: String,
    /// What the box opened with, to tell whether the user typed anything.
    pub original: String,
    pub cursor: i32,
}

#[derive(Clone)]
pub(super) struct BriefPage {
    pub root: gtk::Box,
    /// The assistant's fixed links, such as the calendar.
    pub links: adw::WrapBox,
    pub rows: gtk::Box,
    /// Hidden items, kept at the bottom like Inactive Projects in the sidebar.
    pub hidden: gtk::Box,
}

impl BriefPage {
    pub fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        // Links wrap onto more lines in a narrow sidebar.
        let links = adw::WrapBox::new();
        links.set_child_spacing(4);
        links.set_line_spacing(4);
        links.set_margin_start(12);
        links.set_margin_end(12);
        root.append(&links);

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
            links,
            rows,
            hidden,
        }
    }
}

impl Workspace {
    pub(super) fn install_brief_actions(&self) {
        let workspace = self.clone();
        glib::timeout_add_local(BRIEF_TITLE_INTERVAL, move || {
            workspace.refresh_brief_title();
            glib::ControlFlow::Continue
        });
        self.add_target_action("brief-comment", |workspace, id| {
            workspace.open_brief_comment(id);
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
        let action = gio::SimpleAction::new("brief-toggle-hidden", None);
        let workspace = self.clone();
        action.connect_activate(move |_, _| {
            let expanded = !workspace.brief_hidden_expanded.get();
            workspace.brief_hidden_expanded.set(expanded);
            workspace.save_preference(HIDDEN_EXPANDED_PREFERENCE, serde_json::json!(expanded));
            workspace.render_brief();
        });
        self.window.add_action(&action);
        // Only the view changes: no read, hidden or done mark, and not the brief itself.
        self.add_target_action("brief-toggle-section", |workspace, section| {
            let value = {
                let mut collapsed = workspace.brief_collapsed_sections.borrow_mut();
                if !collapsed.remove(section) {
                    collapsed.insert(section.to_owned());
                }
                serde_json::json!(*collapsed)
            };
            workspace.save_preference(COLLAPSED_SECTIONS_PREFERENCE, value);
            workspace.render_brief();
        });
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
        self.save_brief();
    }

    fn save_brief(&self) {
        let value = serde_json::to_value(&*self.brief.borrow()).unwrap_or_default();
        self.save_preference(PREFERENCE_KEY, value);
    }

    /// Opens the item's comment box, or moves to the one already open.
    fn open_brief_comment(&self, id: &str) {
        let original = {
            let brief = self.brief.borrow();
            let Some(item) = brief.items.iter().find(|item| item.id == id) else {
                return;
            };
            item.comment.clone().unwrap_or_default()
        };
        self.brief_comment_drafts
            .borrow_mut()
            .entry(id.to_owned())
            .or_insert_with(|| CommentDraft {
                cursor: original.chars().count() as i32,
                text: original.clone(),
                original,
            });
        if let Some(focus) = GtkWindowExt::focus(&self.window)
            && !self.is_brief_comment(&focus)
        {
            self.brief_comment_return_focus.set(Some(&focus));
        }
        *self.brief_comment_focus.borrow_mut() = Some(id.to_owned());
        self.render_brief();
        self.focus_brief_comment();
    }

    /// Saves the open box. Too long a comment keeps the box open, with its words.
    fn save_brief_comment(&self, id: &str) {
        let Some(text) = self
            .brief_comment_drafts
            .borrow()
            .get(id)
            .map(|draft| draft.text.clone())
        else {
            return;
        };
        let saved = self.brief.borrow_mut().set_comment(id, &text, now_millis());
        let changed = match saved {
            Ok(changed) => changed,
            Err(error) => {
                self.show_error(&error.to_string());
                return;
            }
        };
        self.close_brief_comment(id);
        if changed {
            self.save_brief();
        }
    }

    /// Closes the box and gives the focus back to where it came from.
    fn close_brief_comment(&self, id: &str) {
        let had_focus =
            GtkWindowExt::focus(&self.window).is_some_and(|focus| self.is_brief_comment(&focus));
        self.brief_comment_drafts.borrow_mut().remove(id);
        self.render_brief();
        if had_focus
            && let Some(focus) = self.brief_comment_return_focus.upgrade()
            && focus.root().is_some()
        {
            focus.grab_focus();
        }
    }

    fn is_brief_comment(&self, focus: &gtk::Widget) -> bool {
        let page = &self.changes.brief;
        focus.is::<gtk::TextView>()
            && (focus.is_ancestor(&page.rows) || focus.is_ancestor(&page.hidden))
    }

    /// Closes boxes whose item left the brief, and those an assistant handled
    /// while the user typed nothing, which would otherwise save the old words again.
    fn reconcile_brief_comments(&self) {
        let brief = self.brief.borrow();
        let mut lost = false;
        self.brief_comment_drafts.borrow_mut().retain(|id, draft| {
            let typed = draft.text != draft.original;
            match brief.items.iter().find(|item| &item.id == id) {
                Some(item) => {
                    typed || item.comment.as_deref().unwrap_or_default() == draft.original
                }
                None => {
                    lost |= typed && !draft.text.trim().is_empty();
                    false
                }
            }
        });
        if lost {
            self.show_error("An item left the brief, so your unsaved comment on it is gone");
        }
    }

    /// Puts the focus in the box last opened or typed in, if it is on show.
    fn focus_brief_comment(&self) {
        let Some(id) = self.brief_comment_focus.borrow().clone() else {
            return;
        };
        let page = &self.changes.brief;
        let view = [&page.rows, &page.hidden]
            .into_iter()
            .flat_map(|rows| std::iter::successors(rows.first_child(), |row| row.next_sibling()))
            .find(|row| row.widget_name() == id.as_str())
            .and_then(|row| comment_view(&row));
        if let Some(view) = view {
            view.grab_focus();
        }
    }

    /// The comment under an item: the open box, or else the pending comment.
    fn with_comment(&self, row: gtk::Box, item: &BriefItem) -> gtk::Box {
        let target = item.id.to_variant();
        on_click(&row, &target);
        row.set_cursor_from_name(Some("pointer"));
        row.set_tooltip_text(Some(if item.comment.is_some() {
            "Click to edit your comment"
        } else {
            "Click to comment. Your assistant reads it."
        }));
        let entry = gtk::Box::new(gtk::Orientation::Vertical, 2);
        entry.set_widget_name(&item.id);
        entry.append(&row);
        if self.brief_comment_drafts.borrow().contains_key(&item.id) {
            entry.append(&self.comment_box(&item.id));
        } else if let Some(comment) = &item.comment {
            let label = gtk::Label::new(Some(comment));
            label.set_xalign(0.0);
            label.set_wrap(true);
            label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            label.set_margin_start(COMMENT_MARGIN);
            label.set_margin_end(COMMENT_MARGIN);
            label.set_margin_bottom(4);
            label.add_css_class("brief-comment");
            label.set_tooltip_text(Some(
                "Waiting for your assistant. Click to edit it; save it empty to remove it.",
            ));
            label.set_cursor_from_name(Some("pointer"));
            on_click(&label, &target);
            entry.append(&label);
        }
        entry
    }

    /// Enter saves, Shift+Enter adds a line and Escape closes without saving,
    /// like the comment box on files and diffs.
    fn comment_box(&self, id: &str) -> gtk::Box {
        let drafts = self.brief_comment_drafts.borrow();
        let draft = &drafts[id];
        let view = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .accepts_tab(false)
            .top_margin(4)
            .bottom_margin(4)
            .left_margin(6)
            .right_margin(6)
            .build();
        view.add_css_class("brief-comment-box");
        view.update_property(&[gtk::accessible::Property::Label("Comment")]);
        let buffer = view.buffer();
        buffer.set_text(&draft.text);
        buffer.place_cursor(&buffer.iter_at_offset(draft.cursor));
        let changed_drafts = self.brief_comment_drafts.clone();
        let changed_id = id.to_owned();
        buffer.connect_changed(move |buffer| {
            if let Some(draft) = changed_drafts.borrow_mut().get_mut(&changed_id) {
                draft.text = buffer
                    .text(&buffer.start_iter(), &buffer.end_iter(), false)
                    .to_string();
            }
        });
        let cursor_drafts = self.brief_comment_drafts.clone();
        let cursor_id = id.to_owned();
        buffer.connect_cursor_position_notify(move |buffer| {
            if let Some(draft) = cursor_drafts.borrow_mut().get_mut(&cursor_id) {
                draft.cursor = buffer.cursor_position();
            }
        });
        // An input method uses Enter and Escape itself while it composes.
        let composing = Rc::new(Cell::new(false));
        let preedit = composing.clone();
        view.connect_preedit_changed(move |_, text| preedit.set(!text.is_empty()));
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let workspace = self.clone();
        let key_id = id.to_owned();
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            if composing.get() {
                return glib::Propagation::Proceed;
            }
            match key {
                gtk::gdk::Key::Return | gtk::gdk::Key::KP_Enter
                    if !modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK) =>
                {
                    workspace.save_brief_comment(&key_id);
                    glib::Propagation::Stop
                }
                gtk::gdk::Key::Escape => {
                    workspace.close_brief_comment(&key_id);
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        view.add_controller(keys);
        let focus = gtk::EventControllerFocus::new();
        let focused = self.brief_comment_focus.clone();
        let focus_id = id.to_owned();
        focus.connect_enter(move |_| *focused.borrow_mut() = Some(focus_id.clone()));
        view.add_controller(focus);
        let hint = gtk::Label::new(Some("Enter saves, Shift+Enter adds a line, Esc closes"));
        hint.set_xalign(0.0);
        hint.set_wrap(true);
        hint.add_css_class("caption");
        hint.add_css_class("dim-label");
        let comment_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        comment_box.set_margin_start(COMMENT_MARGIN);
        comment_box.set_margin_end(COMMENT_MARGIN);
        comment_box.set_margin_bottom(4);
        comment_box.append(&view);
        comment_box.append(&hint);
        comment_box
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

    fn refresh_brief_title(&self) {
        let title = self.brief.borrow().tab_title(now_millis());
        let page = self.changes.pages.page(&self.changes.brief.root);
        if page.title().as_deref() != Some(title.as_str()) {
            page.set_title(Some(&title));
        }
    }

    pub(super) fn render_brief(&self) {
        let page = &self.changes.brief;
        self.refresh_brief_title();
        self.reconcile_brief_comments();
        // A post must not pull the focus out of the terminal, only keep it in a box.
        let comment_had_focus =
            GtkWindowExt::focus(&self.window).is_some_and(|focus| self.is_brief_comment(&focus));
        for rows in [&page.rows, &page.hidden] {
            while let Some(child) = rows.first_child() {
                rows.remove(&child);
            }
        }
        let brief = self.brief.borrow();
        while let Some(child) = page.links.first_child() {
            page.links.remove(&child);
        }
        for link in &brief.links {
            let button = gtk::LinkButton::with_label(&link.url, &link.label);
            // A link button shows its whole URL by default; the domain is enough.
            button.set_tooltip_text(Some(&match link.domain() {
                Some(domain) => format!("Open {domain}"),
                None => "Open the link".to_owned(),
            }));
            page.links.append(&button);
        }
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
        let collapsed = self.brief_collapsed_sections.borrow();
        for (section, items) in sections {
            let expanded = !collapsed.contains(section);
            page.rows.append(&section_header(section, &items, expanded));
            if expanded {
                for item in items {
                    page.rows.append(&self.with_comment(brief_row(item), item));
                }
            }
        }
        if !hidden.is_empty() {
            let expanded = self.brief_hidden_expanded.get();
            page.hidden.append(&hidden_header(&hidden, expanded));
            if expanded {
                for item in hidden {
                    page.hidden
                        .append(&self.with_comment(hidden_row(item), item));
                }
            }
        }
        if comment_had_focus {
            self.focus_brief_comment();
        }
    }
}

/// Heads a section. Collapsed, it shows how many items it holds, and a dot
/// when a comment under one of them is out of sight.
fn section_header(section: &str, items: &[&BriefItem], expanded: bool) -> gtk::Button {
    let toggle = if expanded {
        collapsible_header(section, None, 0, true)
    } else {
        collapsible_header(section, Some(items.len()), pending_comments(items), false)
    };
    toggle.set_widget_name(&section_widget_name(section));
    toggle.set_action_name(Some("win.brief-toggle-section"));
    toggle.set_action_target_value(Some(&section.to_variant()));
    toggle.set_tooltip_text(Some(&if expanded {
        "Collapse this section".to_owned()
    } else {
        collapsed_tooltip(items)
    }));
    toggle
}

fn pending_comments(items: &[&BriefItem]) -> usize {
    items.iter().filter(|item| item.comment.is_some()).count()
}

fn collapsed_tooltip(items: &[&BriefItem]) -> String {
    let noun = if items.len() == 1 { "item" } else { "items" };
    match pending_comments(items) {
        0 => format!("Show {} {noun}", items.len()),
        1 => format!("Show {} {noun}, 1 comment waiting", items.len()),
        pending => format!("Show {} {noun}, {pending} comments waiting", items.len()),
    }
}

/// Names a section's header widget apart from the item rows, which use item ids.
pub(super) fn section_widget_name(section: &str) -> String {
    format!("brief-section:{section}")
}

/// Heads the hidden items at the bottom; the list under it starts collapsed.
fn hidden_header(items: &[&BriefItem], expanded: bool) -> gtk::Button {
    let waiting = if expanded { 0 } else { pending_comments(items) };
    let toggle = collapsible_header("Hidden", Some(items.len()), waiting, expanded);
    toggle.set_action_name(Some("win.brief-toggle-hidden"));
    toggle.set_tooltip_text(Some(&if expanded {
        "Collapse hidden items".to_owned()
    } else {
        collapsed_tooltip(items)
    }));
    toggle
}

/// A chevron, the name and, when given, a count. Dim like a caption, unless
/// it holds pending comments out of sight.
fn collapsible_header(
    name: &str,
    count: Option<usize>,
    waiting: usize,
    expanded: bool,
) -> gtk::Button {
    let contents = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    if waiting == 0 {
        contents.add_css_class("dim-label");
    }
    let icon = gtk::Image::from_icon_name(if expanded {
        "pan-down-symbolic"
    } else {
        "pan-end-symbolic"
    });
    icon.set_pixel_size(14);
    let label = gtk::Label::new(Some(name));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label.add_css_class("caption-heading");
    contents.append(&icon);
    contents.append(&label);
    if waiting > 0 {
        let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        dot.set_valign(gtk::Align::Center);
        dot.add_css_class("brief-comment-dot");
        contents.append(&dot);
    }
    if let Some(count) = count {
        let count_label = gtk::Label::new(Some(&count.to_string()));
        count_label.add_css_class("numeric");
        contents.append(&count_label);
    }
    let toggle = gtk::Button::builder().child(&contents).build();
    toggle.set_margin_top(10);
    toggle.add_css_class("flat");
    toggle.update_state(&[gtk::accessible::State::Expanded(Some(expanded))]);
    toggle
}

fn hidden_row(item: &BriefItem) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    row.set_margin_start(6);
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
    row.set_margin_start(6);
    let text = gtk::Label::new(Some(&item.text));
    text.set_xalign(0.0);
    text.set_hexpand(true);
    text.set_wrap(true);
    text.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    // Done items are hidden, unless the assistant shows one again.
    if item.done {
        strike_through(&text);
    }
    row.append(&text);
    let target = item.id.to_variant();
    if item.session.is_some() {
        let open = brief_button("utilities-terminal-symbolic", "Go to the agent session");
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

/// Buttons claim their own clicks, so only the rest of the widget opens the box.
fn on_click(widget: &impl IsA<gtk::Widget>, target: &glib::Variant) {
    let click = gtk::GestureClick::new();
    let target = target.clone();
    click.connect_released(move |gesture, _, _, _| {
        if let Some(widget) = gesture.widget() {
            let _ = widget.activate_action("win.brief-comment", Some(&target));
        }
    });
    widget.add_controller(click);
}

/// The text view of an item's open comment box.
fn comment_view(entry: &gtk::Widget) -> Option<gtk::TextView> {
    entry.last_child()?.first_child()?.downcast().ok()
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
