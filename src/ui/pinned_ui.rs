//! The pinned session: one session whose terminal sits in a panel of the right
//! sidebar, under its pages, so it stays in view next to the selected session.

use super::*;

pub(super) const PINNED_SESSION_PREFERENCE: &str = "pinnedSessionId";
/// The panel's share of the right sidebar's height.
pub(super) const PINNED_SHARE_PREFERENCE: &str = "pinnedPanelShare";
const DEFAULT_SHARE: f64 = 0.5;
const MIN_SHARE: f64 = 0.1;
const MAX_SHARE: f64 = 0.9;
/// Room for the header and a few terminal rows.
const MIN_PANEL_HEIGHT: i32 = 120;
/// The divider needs the sidebar's height, which is known only after a layout.
const PLACE_ATTEMPTS: u32 = 20;
const PLACE_INTERVAL: Duration = Duration::from_millis(50);

pub(super) fn pinned_share(value: Option<&serde_json::Value>) -> f64 {
    value
        .and_then(serde_json::Value::as_f64)
        .filter(|share| share.is_finite())
        .map_or(DEFAULT_SHARE, |share| share.clamp(MIN_SHARE, MAX_SHARE))
}

#[derive(Clone)]
pub(super) struct PinnedPanel {
    /// The sidebar's pages above, the panel below.
    pub panes: gtk::Paned,
    pub root: gtk::Box,
    pub title: gtk::Label,
    pub state: gtk::Label,
    pub unpin: gtk::Button,
    /// Holds the pinned session's terminal.
    pub host: gtk::Box,
    pub focus: gtk::EventControllerFocus,
    share: Rc<Cell<f64>>,
    /// Whether the share has been turned into a divider position since the panel showed.
    placed: Rc<Cell<bool>>,
    /// Keep the header's name and state in step with the session's row.
    bindings: Rc<RefCell<Vec<glib::Binding>>>,
}

impl PinnedPanel {
    pub fn new(pages: &impl IsA<gtk::Widget>) -> Self {
        let state = gtk::Label::new(None);
        state.add_css_class("session-state");
        let title = gtk::Label::new(None);
        title.set_xalign(0.0);
        title.set_hexpand(true);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title.add_css_class("heading");
        let unpin = gtk::Button::builder()
            .icon_name("view-pin-symbolic")
            .tooltip_text("Unpin: move this session back to the main area")
            .build();
        unpin.add_css_class("flat");
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        header.add_css_class("pinned-header");
        header.append(&state);
        header.append(&title);
        header.append(&unpin);

        let host = gtk::Box::new(gtk::Orientation::Vertical, 0);
        host.set_vexpand(true);
        let focus = gtk::EventControllerFocus::new();
        host.add_controller(focus.clone());

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.set_size_request(-1, MIN_PANEL_HEIGHT);
        root.append(&header);
        root.append(&host);
        root.set_visible(false);

        let panes = gtk::Paned::new(gtk::Orientation::Vertical);
        panes.set_vexpand(true);
        panes.set_start_child(Some(pages));
        panes.set_end_child(Some(&root));
        panes.set_shrink_start_child(false);
        panes.set_shrink_end_child(false);

        let panel = Self {
            panes,
            root,
            title,
            state,
            unpin,
            host,
            focus,
            share: Rc::new(Cell::new(DEFAULT_SHARE)),
            placed: Rc::new(Cell::new(false)),
            bindings: Rc::default(),
        };
        // A closed sidebar has no height yet, so place the divider when it opens.
        let mapped = panel.clone();
        panel.panes.connect_map(move |_| mapped.place_divider());
        panel
    }

    pub fn set_share(&self, share: f64) {
        self.share.set(share.clamp(MIN_SHARE, MAX_SHARE));
        self.placed.set(false);
        self.place_divider();
    }

    /// The share the divider shows now, once the sidebar has a height.
    pub fn dragged_share(&self) -> Option<f64> {
        let height = self.panes.height();
        if height <= 0 || !self.root.is_visible() {
            return None;
        }
        let share = 1.0 - f64::from(self.panes.position()) / f64::from(height);
        let share = share.clamp(MIN_SHARE, MAX_SHARE);
        self.share.set(share);
        Some(share)
    }

    /// Shows a session's terminal page, named and marked like its sidebar row.
    pub fn show(&self, page: &gtk::Widget, name: &gtk::Label, state: &gtk::Label) {
        self.clear();
        self.host.append(page);
        self.bindings.borrow_mut().extend([
            name.bind_property("label", &self.title, "label")
                .sync_create()
                .build(),
            state
                .bind_property("label", &self.state, "label")
                .sync_create()
                .build(),
        ]);
        self.placed.set(false);
        self.root.set_visible(true);
        self.place_divider();
    }

    /// Lets go of the terminal page and hides the panel.
    pub fn clear(&self) {
        for binding in self.bindings.borrow_mut().drain(..) {
            binding.unbind();
        }
        while let Some(child) = self.host.first_child() {
            self.host.remove(&child);
        }
        self.root.set_visible(false);
    }

    /// Turns the share into a divider position as soon as the sidebar has a height.
    fn place_divider(&self) {
        if self.try_place_divider() {
            return;
        }
        let panel = self.clone();
        let attempts = Cell::new(0);
        glib::timeout_add_local(PLACE_INTERVAL, move || {
            attempts.set(attempts.get() + 1);
            if panel.try_place_divider() || attempts.get() >= PLACE_ATTEMPTS {
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    /// True when there is nothing left to place.
    fn try_place_divider(&self) -> bool {
        if self.placed.get() || !self.root.is_visible() || !self.panes.is_mapped() {
            return true;
        }
        let height = self.panes.height();
        if height <= 0 {
            return false;
        }
        self.panes
            .set_position(((1.0 - self.share.get()) * f64::from(height)).round() as i32);
        self.placed.set(true);
        true
    }
}

impl Workspace {
    pub(super) fn install_pinned_panel(&self) {
        let panel = &self.changes.pinned;
        let workspace = self.clone();
        panel
            .unpin
            .connect_clicked(move |_| drop(workspace.unpin_session(true)));
        // Being in the panel's terminal is what counts as having seen its turn.
        let workspace = self.clone();
        panel
            .focus
            .connect_enter(move |_| workspace.acknowledge_pinned_session());
        let workspace = self.clone();
        let handle_dragged = watch_handle_drags(&panel.panes);
        panel.panes.connect_position_notify(move |_| {
            // Resizing the window moves the divider too; only a drag is a choice.
            if handle_dragged.get()
                && let Some(share) = workspace.changes.pinned.dragged_share()
            {
                workspace.save_preference(PINNED_SHARE_PREFERENCE, serde_json::json!(share));
            }
        });
        self.add_target_action("session-pin", |workspace, id| {
            if workspace.is_pinned(id) {
                workspace.unpin_session(true);
            } else {
                workspace.pin_session(id, true);
            }
        });
    }

    pub(super) fn pinned_session_id(&self) -> Option<String> {
        self.pinned_session.borrow().clone()
    }

    pub(super) fn is_pinned(&self, id: &str) -> bool {
        self.pinned_session.borrow().as_deref() == Some(id)
    }

    /// Whether the user is typing in the pinned session's panel.
    pub(super) fn pinned_has_focus(&self) -> bool {
        self.pinned_session.borrow().is_some()
            && self.window.is_active()
            && self.changes.pinned.focus.contains_focus()
    }

    /// The session keyboard actions are about: the pinned one while its panel
    /// has the focus, otherwise the selected one.
    pub(super) fn focused_session_id(&self) -> Option<String> {
        if self.pinned_has_focus() {
            self.pinned_session_id()
        } else {
            self.selected_session_id()
        }
    }

    /// Moves a session's terminal from the main area to the panel in the right
    /// sidebar. It replaces a session pinned before. `reveal` also opens the
    /// sidebar. False when there is no such session.
    pub(super) fn pin_session(&self, id: &str, reveal: bool) -> bool {
        if self.is_pinned(id) {
            if reveal {
                self.reveal_pinned_panel();
            }
            return true;
        }
        let view = self.sessions.borrow().get(id).map(|session| {
            (
                session.page.clone(),
                session.row.clone(),
                session.label.clone(),
                session.state_label.clone(),
            )
        });
        let Some((page, row, label, state_label)) = view else {
            return false;
        };
        self.unpin_session(false);
        let was_selected = self.selected_session_id().as_deref() == Some(id);
        let fallback = self.detach_workspace_session(id);
        self.stack.remove(&page);
        self.changes.pinned.show(&page, &label, &state_label);
        // The row stays in the list and now leads to the panel.
        self.list.unselect_row(&row);
        row.set_selectable(false);
        *self.pinned_session.borrow_mut() = Some(id.to_owned());
        self.mark_pinned_row(id, true);
        self.apply_session_state(id);
        self.save_preference(PINNED_SESSION_PREFERENCE, serde_json::json!(id));
        if was_selected {
            self.select_another_session(fallback);
        }
        self.render_workspace_tabs();
        if reveal {
            self.reveal_pinned_panel();
        }
        true
    }

    /// Moves the pinned session back to the main area and hides the panel.
    /// `select` also selects it there. Returns the session that was pinned.
    pub(super) fn unpin_session(&self, select: bool) -> Option<String> {
        let id = self.pinned_session.borrow_mut().take()?;
        self.changes.pinned.clear();
        self.save_preference(PINNED_SESSION_PREFERENCE, serde_json::Value::Null);
        let view = self
            .sessions
            .borrow()
            .get(&id)
            .map(|session| (session.page.clone(), session.row.clone()));
        if let Some((page, row)) = view {
            self.stack.add_named(&page, Some(&id));
            row.set_selectable(true);
            self.mark_pinned_row(&id, false);
            self.attach_workspace_session(&id);
            if select || self.selected_session_id().is_none() {
                self.list.select_row(Some(&row));
            }
        }
        Some(id)
    }

    /// The pinned session closed: empty the panel. A relaunch pins its successor.
    pub(super) fn forget_pinned_session(&self) {
        if self.pinned_session.borrow_mut().take().is_some() {
            self.changes.pinned.clear();
            self.save_preference(PINNED_SESSION_PREFERENCE, serde_json::Value::Null);
        }
    }

    /// Brings a session in front: selects its row, or goes to the panel when
    /// it is pinned there. False when there is no such session.
    pub(super) fn show_session(&self, id: &str) -> bool {
        if self.is_pinned(id) {
            self.focus_pinned_session();
            return true;
        }
        let row = self
            .sessions
            .borrow()
            .get(id)
            .map(|session| session.row.clone());
        let Some(row) = row else {
            return false;
        };
        if self.pinned_has_focus() && self.list.selected_row().as_ref() == Some(&row) {
            // Selecting the selected row does nothing, and the keyboard is in the panel.
            self.activate_session(id);
        } else {
            self.list.select_row(Some(&row));
        }
        true
    }

    /// Opens the right sidebar if needed and puts the keyboard in the panel.
    pub(super) fn focus_pinned_session(&self) {
        let Some(id) = self.pinned_session_id() else {
            return;
        };
        let was_open = self.changes.split.shows_sidebar();
        self.reveal_pinned_panel();
        let terminal = self
            .sessions
            .borrow()
            .get(&id)
            .map(|session| session.terminal.clone());
        let Some(terminal) = terminal else {
            return;
        };
        if was_open {
            terminal.grab_focus();
        } else {
            // The terminal takes the focus only once the opening sidebar shows it.
            glib::idle_add_local_once(move || {
                terminal.grab_focus();
            });
        }
    }

    fn reveal_pinned_panel(&self) {
        if !self.changes.split.shows_sidebar() {
            self.changes.split.set_show_sidebar(true);
        }
    }

    /// Marks the row of the pinned session and names what its menu entry does.
    fn mark_pinned_row(&self, id: &str, pinned: bool) {
        let sessions = self.sessions.borrow();
        let Some(session) = sessions.get(id) else {
            return;
        };
        session.pin_mark.set_visible(pinned);
        session.pin_menu.remove_all();
        session.pin_menu.append_item(&pin_menu_item(id, pinned));
    }

    /// Copies the state colors of the pinned session's row to the panel's header.
    pub(super) fn sync_pinned_state(&self, id: &str, state: SessionState) {
        if !self.is_pinned(id) {
            return;
        }
        let label = &self.changes.pinned.state;
        for class in status_ui::STATE_CLASSES {
            label.remove_css_class(class);
        }
        label.add_css_class(status_ui::session_state_css_class(state));
        label.set_tooltip_text(Some(status_ui::session_state_name(state)));
    }

    pub(super) fn handle_pin_control(&self, pending: PendingRequest) {
        match pending.request.command.clone() {
            ControlCommand::SessionPin(params) => {
                if !self.pin_session(&params.session_id, true) {
                    let id = pending.request.id.clone();
                    let _ = pending.respond(session_not_found(id, &params.session_id));
                    return;
                }
            }
            ControlCommand::SessionUnpin => {
                self.unpin_session(false);
            }
            _ => return,
        }
        // Queue behind the preference save so the caller observes a durable pin.
        self.run_io(
            || Ok(()),
            move |workspace, _| {
                let id = pending.request.id.clone();
                let _ = pending.respond(ControlResponse::success(
                    id,
                    serde_json::json!({ "pinnedSessionId": workspace.pinned_session_id() }),
                ));
            },
        );
    }
}

/// The row menu's entry for the panel; the same action pins and unpins.
pub(super) fn pin_menu_item(id: &str, pinned: bool) -> gio::MenuItem {
    menus::targeted_item(
        if pinned {
            "Unpin from Side Panel"
        } else {
            "Pin to Side Panel"
        },
        "win.session-pin",
        id,
    )
}

#[cfg(test)]
mod tests {
    use super::pinned_share;

    #[test]
    fn a_missing_or_invalid_share_gives_the_panel_half_the_sidebar() {
        assert_eq!(pinned_share(None), 0.5);
        assert_eq!(pinned_share(Some(&serde_json::json!("tall"))), 0.5);
    }

    #[test]
    fn a_stored_share_leaves_room_for_both_the_pages_and_the_panel() {
        assert_eq!(pinned_share(Some(&serde_json::json!(0.3))), 0.3);
        assert_eq!(pinned_share(Some(&serde_json::json!(0.0))), 0.1);
        assert_eq!(pinned_share(Some(&serde_json::json!(4))), 0.9);
    }
}
