use super::*;

pub(super) const APPEARANCE_PAGE: &str = "appearance";
pub(super) const SHORTCUTS_PAGE: &str = "shortcuts";
const CLAUDE_PAGE: &str = "claude";

/// One libadwaita preferences dialog for appearance, shortcuts and Claude presets.
#[derive(Clone)]
pub(super) struct PreferencesDialog {
    pub(super) modal: modal::Modal,
    dialog: adw::PreferencesDialog,
    theme: adw::ComboRow,
    follow_system: adw::SwitchRow,
    font: adw::EntryRow,
    pub(super) shortcut_entries: Rc<Vec<(ShortcutAction, gtk::Entry)>>,
    shortcut_resets: Rc<Vec<(ShortcutAction, gtk::Button)>>,
    pub(super) claude_presets: adw::EntryRow,
    /// Set while code, not the user, changes a row.
    updating: Rc<Cell<bool>>,
}

impl PreferencesDialog {
    pub(super) fn build(parent: &adw::ApplicationWindow) -> Self {
        let dialog = adw::PreferencesDialog::builder()
            .title("Preferences")
            .content_width(640)
            .content_height(620)
            .build();

        let theme_names = ThemeKey::ALL.map(|key| theme(key).name);
        let theme_row = adw::ComboRow::builder()
            .title("Theme")
            .model(&gtk::StringList::new(&theme_names))
            .build();
        let follow_system = adw::SwitchRow::builder()
            .title("Follow System Style")
            .subtitle("Use the light or dark partner of the theme")
            .build();
        let font = adw::EntryRow::builder()
            .title("Font")
            .show_apply_button(true)
            .build();
        font.set_tooltip_text(Some("A Pango font description, such as Monospace 11"));
        let terminal = adw::PreferencesGroup::builder()
            .title("Terminal")
            .description("Ctrl+plus and Ctrl+minus change the text size")
            .build();
        terminal.add(&theme_row);
        terminal.add(&follow_system);
        terminal.add(&font);
        let appearance = adw::PreferencesPage::builder()
            .name(APPEARANCE_PAGE)
            .title("Appearance")
            .icon_name("preferences-desktop-appearance-symbolic")
            .build();
        appearance.add(&terminal);
        dialog.add(&appearance);

        let shortcuts_group = adw::PreferencesGroup::builder()
            .title("Application Shortcuts")
            .description(
                "Select a shortcut and press a new one with Ctrl, Alt, or Meta. It is saved right away.",
            )
            .build();
        let mut shortcut_entries = Vec::new();
        let mut shortcut_resets = Vec::new();
        for action in ShortcutAction::ALL {
            let entry = gtk::Entry::builder()
                .text(shortcuts_ui::accelerator_label(
                    action.default_accelerator(),
                ))
                .editable(false)
                .width_chars(18)
                .valign(gtk::Align::Center)
                .build();
            entry.set_tooltip_text(Some(&format!(
                "Select, then press the new shortcut for {}",
                action.label()
            )));
            entry.update_property(&[gtk::accessible::Property::Label(action.label())]);
            let reset = gtk::Button::builder()
                .icon_name("edit-undo-symbolic")
                .valign(gtk::Align::Center)
                .tooltip_text(format!("Reset shortcut for {}", action.label()))
                .build();
            reset.add_css_class("flat");
            let row = adw::ActionRow::builder().title(action.label()).build();
            row.add_suffix(&entry);
            row.add_suffix(&reset);
            shortcuts_group.add(&row);
            shortcut_entries.push((action, entry));
            shortcut_resets.push((action, reset));
        }
        let shortcuts = adw::PreferencesPage::builder()
            .name(SHORTCUTS_PAGE)
            .title("Shortcuts")
            .icon_name("preferences-desktop-keyboard-shortcuts-symbolic")
            .build();
        shortcuts.add(&shortcuts_group);
        dialog.add(&shortcuts);

        let claude_presets = adw::EntryRow::builder()
            .title("Presets JSON")
            .show_apply_button(true)
            .build();
        let claude_group = adw::PreferencesGroup::builder()
            .title("Claude Model Presets")
            .description(
                "A JSON array of named model and effort presets. Ctrl+Shift+M cycles through them in a Claude session.",
            )
            .build();
        claude_group.add(&claude_presets);
        let claude = adw::PreferencesPage::builder()
            .name(CLAUDE_PAGE)
            .title("Claude")
            .icon_name("applications-science-symbolic")
            .build();
        claude.add(&claude_group);
        dialog.add(&claude);

        Self {
            modal: modal::Modal::wrap(parent, &dialog, None),
            dialog,
            theme: theme_row,
            follow_system,
            font,
            shortcut_entries: Rc::new(shortcut_entries),
            shortcut_resets: Rc::new(shortcut_resets),
            claude_presets,
            updating: Rc::new(Cell::new(false)),
        }
    }

    pub(super) fn open(&self, page: &str) {
        self.dialog.set_visible_page_name(page);
        self.modal.present();
    }

    pub(super) fn is_showing(&self, page: &str) -> bool {
        self.modal.is_visible() && self.dialog.visible_page_name().as_deref() == Some(page)
    }

    /// Mirrors saved appearance settings without treating them as user edits.
    pub(super) fn show_appearance(&self, preferences: &AppearancePreferences) {
        self.updating.set(true);
        if let Some(index) = ThemeKey::ALL
            .iter()
            .position(|key| *key == preferences.theme)
        {
            self.theme.set_selected(index as u32);
        }
        self.follow_system.set_active(preferences.follow_system);
        if self.font.text().as_str() != preferences.font {
            self.font.set_text(&preferences.font);
        }
        self.updating.set(false);
    }
}

impl Workspace {
    pub(super) fn connect_preferences(&self) {
        let preferences = self.preferences.clone();

        let workspace = self.clone();
        let updating = preferences.updating.clone();
        preferences.theme.connect_selected_notify(move |row| {
            let Some(key) = ThemeKey::ALL.get(row.selected() as usize) else {
                return;
            };
            if !updating.get() {
                workspace.set_appearance(
                    AppearanceSetParams {
                        theme: Some(*key),
                        ..Default::default()
                    },
                    None,
                );
            }
        });

        let workspace = self.clone();
        let updating = preferences.updating.clone();
        preferences.follow_system.connect_active_notify(move |row| {
            if !updating.get() {
                workspace.set_appearance(
                    AppearanceSetParams {
                        follow_system: Some(row.is_active()),
                        ..Default::default()
                    },
                    None,
                );
            }
        });

        let workspace = self.clone();
        preferences.font.connect_apply(move |row| {
            workspace.set_appearance(
                AppearanceSetParams {
                    font: Some(row.text().to_string()),
                    ..Default::default()
                },
                None,
            );
        });

        for (action, entry) in preferences.shortcut_entries.iter() {
            self.install_shortcut_capture(*action, entry);
        }
        for (action, reset) in preferences.shortcut_resets.iter() {
            let workspace = self.clone();
            let action = *action;
            reset.connect_clicked(move |_| {
                workspace.set_shortcut(
                    ShortcutSetParams {
                        action,
                        accelerator: None,
                        reset: true,
                    },
                    None,
                );
            });
        }

        let workspace = self.clone();
        preferences
            .claude_presets
            .connect_apply(move |_| workspace.apply_claude_presets_from_entry());
    }
}
