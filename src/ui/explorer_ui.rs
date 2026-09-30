//! The Files page of the right sidebar: a lazy tree of the worktree plus a flat filter.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use super::*;
use crate::explorer::{FileNode, FileTree, StatusOverlay, filter_files, status_glyph};

pub(super) const FILES_PAGE: &str = "files";
pub(super) const CHANGES_PAGE: &str = "changes";
pub(super) const SIDEBAR_PAGE_PREFERENCE: &str = "changesSidebarPage";

type OpenHandler = Rc<dyn Fn(Arc<FileNode>)>;

#[derive(Clone)]
pub(super) struct FileExplorer {
    pub root: gtk::Box,
    pub filter: gtk::SearchEntry,
    pub views: gtk::Stack,
    pub tree_view: gtk::ListView,
    pub match_view: gtk::ListView,
    pub message: gtk::Label,
    pub footer: gtk::Label,
    tree_store: gio::ListStore,
    tree_model: gtk::TreeListModel,
    match_store: gio::ListStore,
    pub(crate) state: Rc<RefCell<ExplorerState>>,
}

#[derive(Default)]
pub(crate) struct ExplorerState {
    pub worktree_root: Option<PathBuf>,
    pub tree: Option<FileTree>,
    pub overlay: StatusOverlay,
    /// Expanded directories per worktree, restored when its tree is rebuilt.
    expanded_by_root: HashMap<PathBuf, HashSet<Vec<u8>>>,
    pub request_generation: u64,
    /// Rows currently bound in either list, so status marks update in place.
    bound: HashMap<gtk::Widget, BoundRow>,
    open: Option<OpenHandler>,
}

struct BoundRow {
    node: Arc<FileNode>,
    status: gtk::Label,
}

impl FileExplorer {
    pub fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let filter = gtk::SearchEntry::builder()
            .placeholder_text("Filter files")
            .margin_start(12)
            .margin_end(12)
            .margin_bottom(8)
            .build();
        filter.update_property(&[gtk::accessible::Property::Label("Filter files")]);
        root.append(&filter);

        let state = Rc::new(RefCell::new(ExplorerState::default()));
        let tree_store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let tree_model = gtk::TreeListModel::new(tree_store.clone(), false, false, |item| {
            let node = item.downcast_ref::<glib::BoxedAnyObject>()?;
            let node = node.borrow::<Arc<FileNode>>();
            if !node.is_dir {
                return None;
            }
            let children = gio::ListStore::new::<glib::BoxedAnyObject>();
            let items = node
                .children
                .iter()
                .map(|child| glib::BoxedAnyObject::new(child.clone()))
                .collect::<Vec<_>>();
            children.splice(0, 0, &items);
            Some(children.upcast())
        });
        let tree_view = gtk::ListView::new(
            Some(gtk::NoSelection::new(Some(tree_model.clone()))),
            Some(tree_factory(&state)),
        );
        tree_view.set_single_click_activate(true);
        tree_view.add_css_class("navigation-sidebar");
        tree_view.update_property(&[gtk::accessible::Property::Label("Worktree files")]);
        let tree_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&tree_view)
            .build();

        let match_store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let match_view = gtk::ListView::new(
            Some(gtk::NoSelection::new(Some(match_store.clone()))),
            Some(match_factory(&state)),
        );
        match_view.set_single_click_activate(true);
        match_view.add_css_class("navigation-sidebar");
        match_view.update_property(&[gtk::accessible::Property::Label("Matching files")]);
        let match_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&match_view)
            .build();

        let message = gtk::Label::new(Some("Select an agent to browse its worktree"));
        message.set_wrap(true);
        message.set_xalign(0.0);
        message.set_valign(gtk::Align::Start);
        message.set_margin_top(12);
        message.set_margin_start(12);
        message.set_margin_end(12);
        message.add_css_class("dim-label");

        let views = gtk::Stack::builder()
            .vexpand(true)
            .transition_type(gtk::StackTransitionType::None)
            .build();
        views.add_named(&tree_scroll, Some("tree"));
        views.add_named(&match_scroll, Some("matches"));
        views.add_named(&message, Some("message"));
        views.set_visible_child_name("message");
        root.append(&views);

        let footer = gtk::Label::new(None);
        footer.set_xalign(0.0);
        footer.set_margin_start(12);
        footer.set_margin_end(12);
        footer.set_margin_top(4);
        footer.set_margin_bottom(6);
        footer.add_css_class("caption");
        footer.add_css_class("dim-label");
        root.append(&footer);

        let explorer = Self {
            root,
            filter,
            views,
            tree_view,
            match_view,
            message,
            footer,
            tree_store,
            tree_model,
            match_store,
            state,
        };
        explorer.connect_signals();
        explorer
    }

    fn connect_signals(&self) {
        let explorer = self.clone();
        self.tree_view.connect_activate(move |_, position| {
            let Some(row) = explorer.tree_model.row(position) else {
                return;
            };
            let Some(node) = node_of(row.item().as_ref()) else {
                return;
            };
            if node.is_dir {
                row.set_expanded(!row.is_expanded());
            } else {
                explorer.open(node);
            }
        });
        let explorer = self.clone();
        self.match_view.connect_activate(move |_, position| {
            if let Some(node) = node_of(explorer.match_store.item(position).as_ref()) {
                explorer.open(node);
            }
        });
        let explorer = self.clone();
        self.filter
            .connect_search_changed(move |_| explorer.apply_filter());
        let explorer = self.clone();
        self.filter.connect_activate(move |_| {
            // Enter opens the best match without leaving the entry.
            if let Some(node) = node_of(explorer.match_store.item(0).as_ref()) {
                explorer.open(node);
            }
        });
        let filter = self.filter.clone();
        self.filter
            .connect_stop_search(move |_| filter.set_text(""));
        for view in [&self.tree_view, &self.match_view] {
            self.enable_context_menu(view);
        }
    }

    /// Right-clicking a row offers the same file actions the row itself does not show.
    fn enable_context_menu(&self, view: &gtk::ListView) {
        let click = gtk::GestureClick::builder()
            .button(gtk::gdk::BUTTON_SECONDARY)
            .build();
        let explorer = self.clone();
        click.connect_pressed(move |gesture, _, x, y| {
            let Some(view) = gesture.widget() else {
                return;
            };
            let Some(node) = explorer.node_at(&view, x, y) else {
                return;
            };
            gesture.set_state(gtk::EventSequenceState::Claimed);
            let Some(root) = explorer.state.borrow().worktree_root.clone() else {
                return;
            };
            let absolute = root
                .join(OsStr::from_bytes(&node.path))
                .to_string_lossy()
                .into_owned();
            let menu = gio::Menu::new();
            if !node.is_dir {
                menu.append_item(&menus::targeted_item(
                    "Open in Emacs",
                    "win.open-in-emacs",
                    &absolute,
                ));
            }
            menu.append_item(&menus::targeted_item(
                "Copy Path",
                "win.copy-text",
                &node.display_path(),
            ));
            let popover = gtk::PopoverMenu::from_model(Some(&menu));
            popover.set_parent(&view);
            popover.set_has_arrow(false);
            popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            popover.connect_closed(|popover| {
                let popover = popover.clone();
                glib::idle_add_local_once(move || popover.unparent());
            });
            popover.popup();
        });
        view.add_controller(click);
    }

    /// The node whose row is under a pointer position in `view`.
    fn node_at(&self, view: &gtk::Widget, x: f64, y: f64) -> Option<Arc<FileNode>> {
        let mut widget = view.pick(x, y, gtk::PickFlags::DEFAULT);
        let state = self.state.borrow();
        while let Some(current) = widget {
            if let Some(bound) = state.bound.get(&current) {
                return Some(bound.node.clone());
            }
            if &current == view {
                break;
            }
            widget = current.parent();
        }
        None
    }

    pub fn set_open_handler(&self, open: impl Fn(Arc<FileNode>) + 'static) {
        self.state.borrow_mut().open = Some(Rc::new(open));
    }

    fn open(&self, node: Arc<FileNode>) {
        let handler = self.state.borrow().open.clone();
        if let Some(handler) = handler {
            handler(node);
        }
    }

    pub fn set_message(&self, text: &str) {
        self.remember_expansion();
        {
            let mut state = self.state.borrow_mut();
            state.worktree_root = None;
            state.tree = None;
            state.overlay = StatusOverlay::default();
        }
        self.tree_store.remove_all();
        self.match_store.remove_all();
        self.message.set_text(text);
        self.footer.set_text("");
        self.views.set_visible_child_name("message");
    }

    pub fn set_loading(&self) {
        self.set_message("Reading worktree files…");
    }

    /// Shows a freshly listed tree, keeping the widgets when nothing was added or removed.
    pub fn set_tree(&self, root: PathBuf, tree: FileTree) {
        let unchanged = {
            let state = self.state.borrow();
            state.worktree_root.as_ref() == Some(&root)
                && state
                    .tree
                    .as_ref()
                    .is_some_and(|current| current.signature == tree.signature)
        };
        if unchanged {
            self.state.borrow_mut().tree = Some(tree);
            self.show_lists();
            return;
        }
        self.remember_expansion();
        let items = tree
            .roots
            .iter()
            .map(|node| glib::BoxedAnyObject::new(node.clone()))
            .collect::<Vec<_>>();
        self.tree_store.remove_all();
        self.tree_store.splice(0, 0, &items);
        {
            let mut state = self.state.borrow_mut();
            state.worktree_root = Some(root);
            state.tree = Some(tree);
        }
        self.restore_expansion();
        self.apply_filter();
        self.show_lists();
    }

    fn show_lists(&self) {
        let file_count = self
            .state
            .borrow()
            .tree
            .as_ref()
            .map_or(0, |tree| tree.file_count);
        if self.filter.text().trim().is_empty() {
            let files = if file_count == 1 { "file" } else { "files" };
            self.footer.set_text(&format!("{file_count} {files}"));
            self.views.set_visible_child_name("tree");
        } else {
            self.views.set_visible_child_name("matches");
        }
    }

    /// Marks changed files and the folders that hold them without rebuilding rows.
    pub fn set_overlay(&self, overlay: StatusOverlay) {
        let mut state = self.state.borrow_mut();
        if state.overlay == overlay {
            return;
        }
        state.overlay = overlay;
        for bound in state.bound.values() {
            apply_status(&bound.status, &bound.node, &state.overlay);
        }
    }

    fn apply_filter(&self) {
        let query = self.filter.text();
        let query = query.trim();
        let state = self.state.borrow();
        let Some(tree) = state.tree.as_ref() else {
            return;
        };
        if query.is_empty() {
            drop(state);
            self.match_store.remove_all();
            self.show_lists();
            return;
        }
        let (matches, total) = filter_files(tree, query);
        drop(state);
        let items = matches
            .iter()
            .map(|node| glib::BoxedAnyObject::new(node.clone()))
            .collect::<Vec<_>>();
        self.match_store.remove_all();
        self.match_store.splice(0, 0, &items);
        self.footer.set_text(&if total == 0 {
            "No matching files".to_owned()
        } else if total > matches.len() {
            format!("Showing {} of {total} matches", matches.len())
        } else if total == 1 {
            "1 match".to_owned()
        } else {
            format!("{total} matches")
        });
        self.views.set_visible_child_name("matches");
    }

    fn remember_expansion(&self) {
        let Some(root) = self.state.borrow().worktree_root.clone() else {
            return;
        };
        let mut expanded = HashSet::new();
        for position in 0..self.tree_model.n_items() {
            let Some(row) = self.tree_model.row(position) else {
                continue;
            };
            if row.is_expanded()
                && let Some(node) = node_of(row.item().as_ref())
            {
                expanded.insert(node.path.clone());
            }
        }
        self.state
            .borrow_mut()
            .expanded_by_root
            .insert(root, expanded);
    }

    fn restore_expansion(&self) {
        let expanded = {
            let state = self.state.borrow();
            let Some(root) = state.worktree_root.as_ref() else {
                return;
            };
            state.expanded_by_root.get(root).cloned()
        };
        let Some(expanded) = expanded.filter(|expanded| !expanded.is_empty()) else {
            return;
        };
        // Expanding inserts children right below, so a growing walk reaches them too.
        let mut position = 0;
        while position < self.tree_model.n_items() {
            if let Some(row) = self.tree_model.row(position)
                && let Some(node) = node_of(row.item().as_ref())
                && node.is_dir
                && expanded.contains(&node.path)
            {
                row.set_expanded(true);
            }
            position += 1;
        }
    }

    /// The paths of the directories currently expanded, for inspection and tests.
    pub fn expanded_paths(&self) -> Vec<String> {
        (0..self.tree_model.n_items())
            .filter_map(|position| self.tree_model.row(position))
            .filter(|row| row.is_expanded())
            .filter_map(|row| node_of(row.item().as_ref()))
            .map(|node| node.display_path())
            .collect()
    }

    pub fn visible_row_count(&self) -> u32 {
        if self.views.visible_child_name().as_deref() == Some("matches") {
            self.match_store.n_items()
        } else {
            self.tree_model.n_items()
        }
    }
}

fn node_of(item: Option<&glib::Object>) -> Option<Arc<FileNode>> {
    let boxed = item?.downcast_ref::<glib::BoxedAnyObject>()?;
    Some(boxed.borrow::<Arc<FileNode>>().clone())
}

fn tree_factory(state: &Rc<RefCell<ExplorerState>>) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let expander = gtk::TreeExpander::new();
        expander.set_indent_for_icon(true);
        expander.set_child(Some(&row_widget()));
        item.set_child(Some(&expander));
    });
    let bind_state = state.clone();
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(expander) = item.child().and_downcast::<gtk::TreeExpander>() else {
            return;
        };
        let Some(row) = item.item().and_downcast::<gtk::TreeListRow>() else {
            return;
        };
        expander.set_list_row(Some(&row));
        let Some(node) = node_of(row.item().as_ref()) else {
            return;
        };
        if let Some(widget) = expander.child() {
            bind_row(&bind_state, &widget, node, false);
        }
    });
    let unbind_state = state.clone();
    factory.connect_unbind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        if let Some(expander) = item.child().and_downcast::<gtk::TreeExpander>() {
            if let Some(widget) = expander.child() {
                unbind_state.borrow_mut().bound.remove(&widget);
            }
            expander.set_list_row(None);
        }
    });
    factory
}

fn match_factory(state: &Rc<RefCell<ExplorerState>>) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            item.set_child(Some(&row_widget()));
        }
    });
    let bind_state = state.clone();
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let (Some(widget), Some(node)) = (item.child(), node_of(item.item().as_ref())) else {
            return;
        };
        bind_row(&bind_state, &widget, node, true);
    });
    let unbind_state = state.clone();
    factory.connect_unbind(move |_, item| {
        if let Some(widget) = item
            .downcast_ref::<gtk::ListItem>()
            .and_then(gtk::ListItem::child)
        {
            unbind_state.borrow_mut().bound.remove(&widget);
        }
    });
    factory
}

/// Icon, name, an optional dim directory (flat matches only) and a status mark.
fn row_widget() -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let icon = gtk::Image::from_icon_name("text-x-generic-symbolic");
    let name = gtk::Label::new(None);
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    let directory = gtk::Label::new(None);
    directory.set_xalign(0.0);
    directory.set_hexpand(true);
    directory.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    directory.add_css_class("dim-label");
    directory.set_visible(false);
    let status = gtk::Label::new(None);
    status.set_width_chars(1);
    status.add_css_class("accent");
    row.append(&icon);
    row.append(&name);
    row.append(&directory);
    row.append(&status);
    row.upcast()
}

fn bind_row(
    state: &Rc<RefCell<ExplorerState>>,
    widget: &gtk::Widget,
    node: Arc<FileNode>,
    show_directory: bool,
) {
    let Some(icon) = widget.first_child().and_downcast::<gtk::Image>() else {
        return;
    };
    let Some(name) = icon.next_sibling().and_downcast::<gtk::Label>() else {
        return;
    };
    let Some(directory) = name.next_sibling().and_downcast::<gtk::Label>() else {
        return;
    };
    let Some(status) = directory.next_sibling().and_downcast::<gtk::Label>() else {
        return;
    };
    icon.set_icon_name(Some(if node.is_dir {
        "folder-symbolic"
    } else {
        "text-x-generic-symbolic"
    }));
    name.set_text(&node.name);
    name.set_hexpand(!show_directory);
    let path = node.display_path();
    widget.set_tooltip_text(Some(&path));
    if show_directory {
        let parent = path
            .rsplit_once('/')
            .map(|(parent, _)| parent)
            .unwrap_or("");
        directory.set_text(parent);
        directory.set_visible(!parent.is_empty());
    } else {
        directory.set_visible(false);
    }
    let mut state = state.borrow_mut();
    apply_status(&status, &node, &state.overlay);
    state
        .bound
        .insert(widget.clone(), BoundRow { node, status });
}

fn apply_status(label: &gtk::Label, node: &FileNode, overlay: &StatusOverlay) {
    let (text, tooltip) = if node.is_dir {
        if overlay.dirs.contains(&node.path) {
            ("•", Some("Contains changes"))
        } else {
            ("", None)
        }
    } else {
        match overlay.files.get(&node.path) {
            Some(status) => (status_glyph(*status), Some("Changed in this worktree")),
            None => ("", None),
        }
    };
    label.set_text(text);
    label.set_tooltip_text(tooltip);
    if node.is_dir {
        label.remove_css_class("accent");
        label.add_css_class("dim-label");
    } else {
        label.remove_css_class("dim-label");
        label.add_css_class("accent");
    }
}

impl Workspace {
    pub(super) fn connect_explorer(&self) {
        let workspace = self.clone();
        self.changes
            .explorer
            .set_open_handler(move |node| workspace.open_explorer_file(&node));
        let workspace = self.clone();
        self.changes
            .pages
            .connect_visible_child_name_notify(move |pages| {
                let page = pages
                    .visible_child_name()
                    .map(|name| name.to_string())
                    .unwrap_or_default();
                workspace.save_preference(SIDEBAR_PAGE_PREFERENCE, serde_json::json!(page));
                if page == FILES_PAGE {
                    workspace.refresh_explorer_from_snapshot();
                    workspace.changes.explorer.filter.grab_focus();
                }
            });
    }

    fn refresh_explorer_from_snapshot(&self) {
        let snapshot = self.changes.state.borrow().snapshot.clone();
        match snapshot {
            Some(snapshot) => self.refresh_explorer(&snapshot),
            None => self
                .changes
                .explorer
                .set_message(&self.changes.branch.text()),
        }
    }

    /// Relists the worktree while the Files page shows; status marks update either way.
    pub(super) fn refresh_explorer(&self, snapshot: &crate::changes::ChangesSnapshot) {
        let explorer = &self.changes.explorer;
        explorer.set_overlay(StatusOverlay::from_snapshot(snapshot));
        if !self.changes.split.shows_sidebar()
            || self.changes.pages.visible_child_name().as_deref() != Some(FILES_PAGE)
        {
            return;
        }
        let root = snapshot.context.root.clone();
        let generation = {
            let mut state = explorer.state.borrow_mut();
            state.request_generation = state.request_generation.wrapping_add(1);
            if state.worktree_root.as_ref() != Some(&root) {
                drop(state);
                explorer.set_loading();
            }
            explorer.state.borrow().request_generation
        };
        let listed_root = root.clone();
        let result = self.changes_io.submit(move || {
            crate::changes::list_worktree_files(&listed_root)
                .map(|paths| crate::explorer::build_tree(paths))
        });
        let workspace = self.clone();
        glib::spawn_future_local(async move {
            let result = result
                .await
                .unwrap_or_else(|_| Err("Changes worker stopped".to_owned()));
            let explorer = &workspace.changes.explorer;
            if explorer.state.borrow().request_generation != generation {
                return;
            }
            match result {
                Ok(tree) => explorer.set_tree(root, tree),
                Err(error) => explorer.set_message(&format!("Could not list files: {error}")),
            }
        });
    }

    fn open_explorer_file(&self, node: &FileNode) {
        let Some(root) = self.changes.explorer.state.borrow().worktree_root.clone() else {
            return;
        };
        let Some(owner) = self.selected_session_id() else {
            return;
        };
        let worktree_root = root.to_string_lossy().into_owned();
        if self.workspace_tabs.borrow().active_context() != Some(worktree_root.as_str()) {
            self.show_error("The file list belongs to another worktree; refresh and try again");
            return;
        }
        let key = crate::workspace_tabs::FileTabKey {
            worktree_root,
            path: root.join(OsStr::from_bytes(&node.path)),
        };
        self.open_file_tab(key, owner, None);
    }
}
