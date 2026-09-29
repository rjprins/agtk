use super::*;
use crate::appearance::{DEFAULT_UI_FONT_SIZE, MAX_UI_FONT_SIZE, MIN_UI_FONT_SIZE};

pub(super) const APPEARANCE_PAGE: &str = "appearance";
pub(super) const SHORTCUTS_PAGE: &str = "shortcuts";
const CLAUDE_PAGE: &str = "claude";

/// One libadwaita preferences dialog for appearance, shortcuts and Claude presets.
#[derive(Clone)]
pub(super) struct PreferencesDialog {
    pub(super) modal: modal::Modal,
    pages: adw::ViewStack,
    theme: adw::ComboRow,
    follow_system: adw::SwitchRow,
    font: adw::EntryRow,
    text_size: adw::SpinRow,
    pub(super) shortcut_entries: Rc<Vec<(ShortcutAction, gtk::Entry)>>,
    shortcut_resets: Rc<Vec<(ShortcutAction, gtk::Button)>>,
    pub(super) claude_presets: adw::EntryRow,
    /// Set while code, not the user, changes a row.
    updating: Rc<Cell<bool>>,
}

impl PreferencesDialog {
    pub(super) fn build(parent: &adw::ApplicationWindow) -> Self {
        let pages = adw::ViewStack::new();

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
        let terminal = adw::PreferencesGroup::builder().title("Terminal").build();
        terminal.add(&theme_row);
        terminal.add(&follow_system);
        terminal.add(&font);
        let text_size = adw::SpinRow::builder()
            .title("Interface Text Size")
            .subtitle(format!(
                "{DEFAULT_UI_FONT_SIZE} matches the system. Ctrl+plus and Ctrl+minus change it with the terminal font."
            ))
            .adjustment(&gtk::Adjustment::new(
                f64::from(DEFAULT_UI_FONT_SIZE),
                f64::from(MIN_UI_FONT_SIZE),
                f64::from(MAX_UI_FONT_SIZE),
                1.0,
                1.0,
                0.0,
            ))
            .build();
        let interface = adw::PreferencesGroup::builder().title("Interface").build();
        interface.add(&text_size);
        let appearance = adw::PreferencesPage::builder()
            .name(APPEARANCE_PAGE)
            .title("Appearance")
            .icon_name("preferences-desktop-appearance-symbolic")
            .build();
        appearance.add(&terminal);
        appearance.add(&interface);
        pages.add_titled_with_icon(
            &appearance,
            Some(APPEARANCE_PAGE),
            &appearance.title(),
            "preferences-desktop-appearance-symbolic",
        );

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
        pages.add_titled_with_icon(
            &shortcuts,
            Some(SHORTCUTS_PAGE),
            &shortcuts.title(),
            "preferences-desktop-keyboard-shortcuts-symbolic",
        );

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
        pages.add_titled_with_icon(
            &claude,
            Some(CLAUDE_PAGE),
            &claude.title(),
            "applications-science-symbolic",
        );

        let modal = modal::Modal::new(parent, "Preferences", 640, 620, &pages);
        let switcher = adw::ViewSwitcher::builder()
            .stack(&pages)
            .policy(adw::ViewSwitcherPolicy::Wide)
            .build();
        modal.header().set_title_widget(Some(&switcher));

        Self {
            modal,
            pages,
            theme: theme_row,
            follow_system,
            font,
            text_size,
            shortcut_entries: Rc::new(shortcut_entries),
            shortcut_resets: Rc::new(shortcut_resets),
            claude_presets,
            updating: Rc::new(Cell::new(false)),
        }
    }

    pub(super) fn open(&self, page: &str) {
        self.pages.set_visible_child_name(page);
        self.modal.present();
    }

    pub(super) fn is_showing(&self, page: &str) -> bool {
        self.modal.is_visible() && self.pages.visible_child_name().as_deref() == Some(page)
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
        self.text_size
            .set_value(f64::from(preferences.ui_font_size));
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
        let updating = preferences.updating.clone();
        preferences.text_size.connect_value_notify(move |row| {
            if updating.get() {
                return;
            }
            let size = row.value().round() as u8;
            if size != workspace.appearance.borrow().ui_font_size {
                workspace.set_appearance(
                    AppearanceSetParams {
                        ui_font_size: Some(size),
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
