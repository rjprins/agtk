use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;

use crate::changes::{
    ChangeStatus, ChangedFile, ChangesSnapshot, CommitSummary, DiffScope, display_changed_path,
};

const INITIAL_FILE_LIMIT: usize = 200;
const COMMITS_AGO_OPTION: &str = "Commits ago…";

#[derive(Clone)]
pub(super) struct ChangesSidebar {
    pub split: adw::OverlaySplitView,
    pub toggle: gtk::ToggleButton,
    pub root: gtk::Box,
    pub header: gtk::Label,
    pub branch: gtk::Label,
    pub base: gtk::DropDown,
    pub commits_ago: gtk::SpinButton,
    pub commits_ago_row: gtk::Box,
    pub comparison_hint: gtk::Label,
    pub refresh: gtk::Button,
    pub rows: gtk::Box,
    pub(crate) state: Rc<RefCell<ChangesSidebarState>>,
    pub updating_base: Rc<std::cell::Cell<bool>>,
}

#[derive(Default)]
pub(crate) struct ChangesSidebarState {
    pub context_key: Option<String>,
    pub request_generation: u64,
    pub refresh_in_flight: bool,
    pub refresh_pending: bool,
    pub snapshot: Option<ChangesSnapshot>,
    pub loaded_commits: HashMap<String, Vec<ChangedFile>>,
    pub commit_containers: HashMap<String, gtk::Box>,
    pub active_expansion_context: Option<String>,
    pub expansion_by_context: HashMap<String, HashMap<String, bool>>,
    pub base_refs: Vec<String>,
    pub base_ref_by_context: HashMap<String, String>,
}

impl ChangesSidebar {
    pub fn new(center: &impl IsA<gtk::Widget>) -> Self {
        let split = adw::OverlaySplitView::new();
        split.set_content(Some(center));
        split.set_sidebar_position(gtk::PackType::End);
        split.set_sidebar_width_unit(adw::LengthUnit::Sp);
        split.set_sidebar_width_fraction(0.28);
        split.set_min_sidebar_width(240.0);
        split.set_max_sidebar_width(420.0);
        split.set_show_sidebar(false);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("sidebar-pane");
        root.set_size_request(240, -1);
        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        toolbar.set_margin_top(8);
        toolbar.set_margin_bottom(8);
        toolbar.set_margin_start(12);
        toolbar.set_margin_end(12);
        let header = gtk::Label::new(Some("Changes"));
        header.add_css_class("heading");
        header.set_xalign(0.0);
        header.set_hexpand(true);
        let refresh = gtk::Button::builder()
            .icon_name("view-refresh-symbolic")
            .tooltip_text("Refresh changes")
            .build();
        refresh.add_css_class("flat");
        let close = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Close Changes sidebar")
            .build();
        close.add_css_class("flat");
        toolbar.append(&header);
        toolbar.append(&refresh);
        toolbar.append(&close);
        root.append(&toolbar);

        let branch = gtk::Label::new(Some("Select an agent to inspect its worktree"));
        branch.set_xalign(0.0);
        branch.set_wrap(true);
        branch.set_margin_start(12);
        branch.set_margin_end(12);
        branch.set_margin_bottom(8);
        branch.add_css_class("dim-label");
        root.append(&branch);

        let base_label = gtk::Label::new(Some("Compared with"));
        base_label.set_xalign(0.0);
        base_label.set_margin_start(12);
        base_label.set_margin_end(12);
        base_label.add_css_class("caption");
        root.append(&base_label);
        let base = gtk::DropDown::new(
            Some(gtk::StringList::new(&["Choose comparison base"])),
            None::<&gtk::Expression>,
        );
        base.set_margin_start(12);
        base.set_margin_end(12);
        base.set_margin_bottom(8);
        base.set_tooltip_text(Some("Compare with a branch or an earlier commit"));
        root.append(&base);

        let commits_ago_row = gtk::Box::new(gtk::Orientation::Vertical, 4);
        commits_ago_row.set_margin_start(12);
        commits_ago_row.set_margin_end(12);
        commits_ago_row.set_margin_bottom(8);
        let count_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let count_label = gtk::Label::new(Some("Commits ago"));
        count_label.set_xalign(0.0);
        count_label.set_hexpand(true);
        let commits_ago = gtk::SpinButton::with_range(1.0, i32::MAX as f64, 1.0);
        commits_ago.set_numeric(true);
        commits_ago.set_width_chars(4);
        commits_ago.set_max_width_chars(10);
        commits_ago.set_value(1.0);
        commits_ago.update_property(&[gtk::accessible::Property::Label("Commits ago")]);
        commits_ago.set_tooltip_text(Some(
            "Number of commits before HEAD, following the first parent",
        ));
        count_row.append(&count_label);
        count_row.append(&commits_ago);
        commits_ago_row.append(&count_row);
        let comparison_hint = gtk::Label::new(None);
        comparison_hint.set_xalign(0.0);
        comparison_hint.set_wrap(true);
        comparison_hint.add_css_class("caption");
        comparison_hint.add_css_class("dim-label");
        commits_ago_row.append(&comparison_hint);
        commits_ago_row.set_visible(false);
        root.append(&commits_ago_row);

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
        split.set_sidebar(Some(&root));

        let toggle = gtk::ToggleButton::with_label("Changes");
        toggle.add_css_class("flat");
        toggle.set_tooltip_text(Some("Show changes for the selected agent's worktree"));
        let sync_toggle = toggle.clone();
        split.connect_show_sidebar_notify(move |split| {
            sync_toggle.set_active(split.shows_sidebar());
        });
        let split_copy = split.clone();
        toggle.connect_toggled(move |toggle| split_copy.set_show_sidebar(toggle.is_active()));
        let split_copy = split.clone();
        close.connect_clicked(move |_| split_copy.set_show_sidebar(false));

        Self {
            split,
            toggle,
            root,
            header,
            branch,
            base,
            commits_ago,
            commits_ago_row,
            comparison_hint,
            refresh,
            rows,
            state: Rc::new(RefCell::new(ChangesSidebarState::default())),
            updating_base: Rc::new(std::cell::Cell::new(false)),
        }
    }

    pub fn set_loading(&self, text: &str) {
        self.branch.set_text(text);
        {
            let mut state = self.state.borrow_mut();
            state.snapshot = None;
            state.loaded_commits.clear();
            state.commit_containers.clear();
        }
        clear(&self.rows);
        let spinner = gtk::Spinner::new();
        spinner.start();
        spinner.set_halign(gtk::Align::Center);
        spinner.set_margin_top(32);
        self.rows.append(&spinner);
    }

    pub fn set_error(&self, text: &str) {
        self.branch.set_text(text);
        {
            let mut state = self.state.borrow_mut();
            state.snapshot = None;
            state.loaded_commits.clear();
            state.commit_containers.clear();
        }
        clear(&self.rows);
        let message = gtk::Label::new(Some(text));
        message.set_wrap(true);
        message.set_margin_top(24);
        message.set_margin_start(12);
        message.set_margin_end(12);
        self.rows.append(&message);
    }

    pub fn set_base_refs(&self, refs: Vec<String>, selected: Option<&str>) {
        let count = selected.and_then(crate::changes::commits_ago_count);
        let selected = selected.unwrap_or("Choose comparison base");
        let selected = if count.is_some() {
            COMMITS_AGO_OPTION
        } else {
            selected
        };
        let mut options = vec![
            "Choose comparison base".to_owned(),
            COMMITS_AGO_OPTION.to_owned(),
        ];
        for reference in refs {
            if !options.contains(&reference) {
                options.push(reference);
            }
        }
        if !options.iter().any(|reference| reference == selected) {
            options.push(selected.to_owned());
        }
        let strings = options.iter().map(String::as_str).collect::<Vec<_>>();
        self.updating_base.set(true);
        if let Some(count) = count
            && self.commits_ago.value_as_int() != count
        {
            self.commits_ago.set_value(f64::from(count));
        }
        self.base.set_model(Some(&gtk::StringList::new(&strings)));
        self.base.set_selected(
            options
                .iter()
                .position(|reference| reference == selected)
                .unwrap_or(0) as u32,
        );
        self.state.borrow_mut().base_refs = options;
        self.update_comparison_hint();
        self.updating_base.set(false);
    }

    pub fn update_comparison_hint(&self) {
        let relative = self.base.selected() == 1;
        self.commits_ago_row.set_visible(relative);
        let count = self.commits_ago.value_as_int();
        let unit = if count == 1 { "commit" } else { "commits" };
        self.comparison_hint
            .set_text(&format!("Changes since {count} {unit} ago"));
    }

    pub fn selected_base(&self) -> Option<String> {
        let index = self.base.selected() as usize;
        if index == 0 {
            return None;
        }
        if index == 1 {
            return Some(format!("HEAD~{}", self.commits_ago.value_as_int()));
        }
        self.state.borrow().base_refs.get(index).cloned()
    }

    pub fn set_base_preference(&self, context: &str, selected: Option<String>) {
        let mut state = self.state.borrow_mut();
        if let Some(selected) = selected {
            state
                .base_ref_by_context
                .insert(context.to_owned(), selected);
        } else {
            state.base_ref_by_context.remove(context);
        }
    }

    pub fn base_preferences(&self) -> HashMap<String, String> {
        self.state.borrow().base_ref_by_context.clone()
    }

    pub fn render(
        &self,
        snapshot: ChangesSnapshot,
        open_file: impl Fn(ChangedFile) + Clone + 'static,
        load_commit: impl Fn(String, gtk::Box) + Clone + 'static,
        load_more: impl Fn() + Clone + 'static,
    ) {
        let branch = if snapshot.context.detached {
            format!("Detached HEAD at {}", short(&snapshot.context.head_oid))
        } else {
            snapshot
                .context
                .branch
                .clone()
                .unwrap_or_else(|| "Unborn repository".to_owned())
        };
        self.branch.set_text(&branch);
        self.branch
            .set_tooltip_text(Some(&snapshot.context.root.to_string_lossy()));
        self.base.set_sensitive(!snapshot.context.unborn);
        self.commits_ago.set_sensitive(!snapshot.context.unborn);
        {
            let mut state = self.state.borrow_mut();
            state.active_expansion_context =
                Some(snapshot.context.root.to_string_lossy().into_owned());
            state.snapshot = Some(snapshot.clone());
        }
        clear(&self.rows);
        for warning in &snapshot.warnings {
            let message = gtk::Label::new(Some(warning));
            message.set_wrap(true);
            message.set_xalign(0.0);
            message.set_margin_start(8);
            message.set_margin_end(8);
            message.set_margin_bottom(8);
            self.rows.append(&message);
        }

        let all_expanded = self.expanded("all", true);
        self.add_group(
            "all",
            "All changes",
            snapshot.all_changes.clone(),
            all_expanded,
            DiffScope::All,
            open_file.clone(),
        );
        let uncommitted_files = snapshot
            .staged
            .iter()
            .chain(snapshot.unstaged.iter())
            .chain(snapshot.untracked.iter())
            .chain(snapshot.conflicts.iter())
            .flat_map(|file| [file.old_path.as_ref(), file.new_path.as_ref()])
            .flatten()
            .cloned()
            .collect::<std::collections::HashSet<_>>()
            .len();
        let uncommitted = gtk::Expander::new(Some(&format!("Uncommitted ({uncommitted_files})")));
        uncommitted.set_expanded(self.expanded("uncommitted", true));
        self.remember_expansion(&uncommitted, "uncommitted");
        self.rows.append(&uncommitted);
        let uncommitted_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        self.append_group(
            &uncommitted_box,
            "staged",
            "Staged",
            snapshot.staged.clone(),
            self.expanded("staged", false),
            DiffScope::Staged,
            open_file.clone(),
        );
        self.append_group(
            &uncommitted_box,
            "unstaged",
            "Unstaged",
            snapshot.unstaged.clone(),
            self.expanded("unstaged", false),
            DiffScope::Unstaged,
            open_file.clone(),
        );
        self.append_group(
            &uncommitted_box,
            "untracked",
            "Untracked",
            snapshot.untracked.clone(),
            self.expanded("untracked", false),
            DiffScope::Untracked,
            open_file.clone(),
        );
        if !snapshot.conflicts.is_empty() {
            self.append_group(
                &uncommitted_box,
                "conflicts",
                "Conflicts",
                snapshot.conflicts.clone(),
                self.expanded("conflicts", true),
                DiffScope::All,
                open_file.clone(),
            );
        }
        uncommitted.set_child(Some(&uncommitted_box));

        self.add_group(
            "committed",
            if snapshot
                .context
                .base_ref
                .as_deref()
                .and_then(crate::changes::commits_ago_count)
                .is_some()
            {
                "Committed since comparison"
            } else {
                "Committed on branch"
            },
            snapshot.branch_changes.clone(),
            self.expanded("committed", false),
            DiffScope::Committed,
            open_file.clone(),
        );
        self.add_commits(
            &snapshot.commits,
            snapshot.has_more_commits,
            open_file,
            load_commit,
            load_more,
        );
        if snapshot.all_changes.is_empty()
            && snapshot.staged.is_empty()
            && snapshot.unstaged.is_empty()
            && snapshot.untracked.is_empty()
            && snapshot.branch_changes.is_empty()
            && snapshot.commits.is_empty()
            && snapshot.warnings.is_empty()
        {
            let empty = gtk::Label::new(Some(
                snapshot
                    .context
                    .comparison_warning
                    .as_deref()
                    .unwrap_or("No changes in this worktree"),
            ));
            empty.set_wrap(true);
            empty.set_xalign(0.0);
            empty.set_margin_top(12);
            empty.set_margin_start(8);
            self.rows.prepend(&empty);
        }
    }

    fn add_group(
        &self,
        key: &str,
        title: &str,
        files: Vec<ChangedFile>,
        expanded: bool,
        scope: DiffScope,
        open_file: impl Fn(ChangedFile) + Clone + 'static,
    ) {
        self.append_group(&self.rows, key, title, files, expanded, scope, open_file);
    }

    fn append_group(
        &self,
        parent: &gtk::Box,
        key: &str,
        title: &str,
        files: Vec<ChangedFile>,
        expanded: bool,
        scope: DiffScope,
        open_file: impl Fn(ChangedFile) + Clone + 'static,
    ) {
        let expander = append_file_group(parent, title, files, expanded, scope, open_file);
        self.remember_expansion(&expander, key);
    }

    fn remember_expansion(&self, expander: &gtk::Expander, key: &str) {
        let expansion = self.state.clone();
        let key = key.to_owned();
        let context = self
            .state
            .borrow()
            .active_expansion_context
            .clone()
            .unwrap_or_else(|| "session".to_owned());
        expander.connect_expanded_notify(move |expander| {
            expansion
                .borrow_mut()
                .expansion_by_context
                .entry(context.clone())
                .or_default()
                .insert(key.clone(), expander.is_expanded());
        });
    }

    fn add_commits(
        &self,
        commits: &[CommitSummary],
        has_more: bool,
        open_file: impl Fn(ChangedFile) + Clone + 'static,
        load_commit: impl Fn(String, gtk::Box) + Clone + 'static,
        load_more: impl Fn() + Clone + 'static,
    ) {
        let title = format!("Commits ({})", commits.len());
        let expander = gtk::Expander::new(Some(&title));
        expander.set_expanded(self.expanded("commits", false));
        self.remember_expansion(&expander, "commits");
        self.rows.append(&expander);
        let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        for commit in commits {
            let child = gtk::Box::new(gtk::Orientation::Vertical, 2);
            let commit_expander = gtk::Expander::new(Some(&commit.subject));
            let commit_expansion_key = format!("commit:{}", commit.oid);
            self.remember_expansion(&commit_expander, &commit_expansion_key);
            commit_expander.set_expanded(self.expanded(&commit_expansion_key, false));
            commit_expander.set_tooltip_text(Some(&format!("{} by {}", commit.oid, commit.author)));
            let files = gtk::Box::new(gtk::Orientation::Vertical, 0);
            let oid = commit.oid.clone();
            let cached = self.state.borrow().loaded_commits.get(&oid).cloned();
            let requested = Rc::new(std::cell::Cell::new(cached.is_some()));
            if let Some(cached) = cached {
                for file in cached {
                    append_file_button(
                        &files,
                        file,
                        DiffScope::Commit { oid: oid.clone() },
                        open_file.clone(),
                    );
                }
            } else {
                files.append(&gtk::Label::new(Some("Expand to load changed files")));
            }
            let files_for_load = files.clone();
            let load_commit = load_commit.clone();
            let requested_once = requested.clone();
            commit_expander.connect_expanded_notify(move |expander| {
                if expander.is_expanded() && !requested_once.replace(true) {
                    while let Some(child) = files_for_load.first_child() {
                        files_for_load.remove(&child);
                    }
                    files_for_load.append(&gtk::Label::new(Some("Loading changed files…")));
                    load_commit(oid.clone(), files_for_load.clone());
                }
            });
            commit_expander.set_child(Some(&files));
            child.append(&commit_expander);
            list.append(&child);
            self.state
                .borrow_mut()
                .commit_containers
                .insert(commit.oid.clone(), files);
        }
        if has_more {
            let more = gtk::Button::with_label("Load next 50 commits");
            more.add_css_class("flat");
            more.set_tooltip_text(Some("Load the next 50 commits in this branch range"));
            let load_more = load_more.clone();
            more.connect_clicked(move |_| load_more());
            list.append(&more);
        }
        expander.set_child(Some(&list));
    }

    fn expanded(&self, key: &str, default: bool) -> bool {
        let state = self.state.borrow();
        state
            .active_expansion_context
            .as_ref()
            .and_then(|context| state.expansion_by_context.get(context))
            .and_then(|expansion| expansion.get(key))
            .copied()
            .unwrap_or(default)
    }
}

fn append_file_group(
    parent: &gtk::Box,
    title: &str,
    files: Vec<ChangedFile>,
    expanded: bool,
    scope: DiffScope,
    open_file: impl Fn(ChangedFile) + Clone + 'static,
) -> gtk::Expander {
    let expander = gtk::Expander::new(Some(&format!("{title} ({})", files.len())));
    expander.set_expanded(expanded);
    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let shown = files.len().min(INITIAL_FILE_LIMIT);
    for file in files.iter().take(shown).cloned() {
        append_file_button(&list, file, scope.clone(), open_file.clone());
    }
    if files.len() > shown {
        let remaining = files.into_iter().skip(shown).collect::<Vec<_>>();
        let more = gtk::Button::with_label(&format!("Show {} more files", remaining.len()));
        more.add_css_class("flat");
        let list_for_more = list.clone();
        more.connect_clicked(move |button| {
            for file in remaining.iter().cloned() {
                append_file_button(&list_for_more, file, scope.clone(), open_file.clone());
            }
            button.set_visible(false);
        });
        list.append(&more);
    }
    expander.set_child(Some(&list));
    parent.append(&expander);
    expander
}

pub(super) fn append_file_button(
    parent: &gtk::Box,
    mut file: ChangedFile,
    scope: DiffScope,
    open_file: impl Fn(ChangedFile) + Clone + 'static,
) {
    file.scope = scope;
    let path = display_changed_path(&file);
    let status = status_glyph(file.status);
    let label = gtk::Label::new(Some(&format!("{status}  {path}")));
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    label.set_tooltip_text(Some(&path));
    let button = gtk::Button::new();
    button.set_child(Some(&label));
    button.set_halign(gtk::Align::Fill);
    button.add_css_class("flat");
    button.set_accessible_role(gtk::AccessibleRole::Button);
    let file = file.clone();
    button.connect_clicked(move |_| open_file(file.clone()));
    parent.append(&button);
}

fn status_glyph(status: ChangeStatus) -> &'static str {
    match status {
        ChangeStatus::Added | ChangeStatus::Untracked => "+",
        ChangeStatus::Deleted => "−",
        ChangeStatus::Renamed => "↪",
        ChangeStatus::Unmerged => "!",
        _ => "•",
    }
}

fn clear(box_widget: &gtk::Box) {
    while let Some(child) = box_widget.first_child() {
        box_widget.remove(&child);
    }
}

fn short(oid: &Option<String>) -> String {
    oid.as_deref()
        .map(|oid| oid.chars().take(12).collect())
        .unwrap_or_else(|| "unborn".to_owned())
}
