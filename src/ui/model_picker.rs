use super::*;
use crate::agent_models::{self, AgentModels, ModelOption};
use crate::providers::DiscoveryRoots;

const AGENT_DEFAULT: &str = "Agent default";

/// A model dropdown and the effort dropdown that follows the chosen model.
#[derive(Clone)]
pub(super) struct ModelPicker {
    pub(super) model_row: adw::ComboRow,
    pub(super) effort_row: adw::ComboRow,
    options: Rc<RefCell<Vec<ModelOption>>>,
    /// The effort row's values; the first is empty for the agent's default.
    efforts: Rc<RefCell<Vec<String>>>,
    updating: Rc<Cell<bool>>,
}

impl ModelPicker {
    pub(super) fn new() -> Self {
        let model_row = adw::ComboRow::builder().title("Model").build();
        let effort_row = adw::ComboRow::builder().title("Reasoning effort").build();
        let picker = Self {
            model_row,
            effort_row,
            options: Rc::default(),
            efforts: Rc::default(),
            updating: Rc::default(),
        };
        // Holds no strong reference to the rows, so a closed dialog can go.
        let (options, efforts, updating) = (
            picker.options.clone(),
            picker.efforts.clone(),
            picker.updating.clone(),
        );
        let effort_row = picker.effort_row.downgrade();
        picker.model_row.connect_selected_notify(move |model_row| {
            if let Some(effort_row) = effort_row.upgrade()
                && !updating.get()
            {
                let keep = selected_effort(&effort_row, &efforts);
                refresh_efforts(model_row, &effort_row, &options, &efforts, &keep);
            }
        });
        picker
    }

    pub(super) fn set_options(&self, options: Vec<ModelOption>) {
        let (model, effort) = (self.model(), self.effort());
        *self.options.borrow_mut() = options;
        self.set_model(&model);
        self.set_effort(&effort);
    }

    pub(super) fn model(&self) -> String {
        self.options
            .borrow()
            .get(self.model_row.selected() as usize)
            .map(|option| option.value.clone())
            .unwrap_or_default()
    }

    /// Selects a model, keeping one the list does not know, such as one
    /// saved before the agent dropped it, rather than losing it.
    pub(super) fn set_model(&self, value: &str) {
        let effort = self.effort();
        let index = {
            let mut options = self.options.borrow_mut();
            if !value.is_empty() && !options.iter().any(|option| option.value == value) {
                let efforts = options
                    .first()
                    .map(|option| option.efforts.clone())
                    .unwrap_or_default();
                options.push(ModelOption {
                    value: value.to_owned(),
                    label: value.to_owned(),
                    efforts,
                });
            }
            let labels = options
                .iter()
                .map(|option| option.label.as_str())
                .collect::<Vec<_>>();
            self.updating.set(true);
            self.model_row
                .set_model(Some(&gtk::StringList::new(&labels)));
            self.updating.set(false);
            options
                .iter()
                .position(|option| option.value == value)
                .unwrap_or(0)
        };
        self.updating.set(true);
        self.model_row.set_selected(index as u32);
        self.updating.set(false);
        self.refresh_efforts(&effort);
    }

    pub(super) fn effort(&self) -> String {
        selected_effort(&self.effort_row, &self.efforts)
    }

    pub(super) fn set_effort(&self, value: &str) {
        select_effort(&self.effort_row, &self.efforts, value);
    }

    fn refresh_efforts(&self, keep: &str) {
        refresh_efforts(
            &self.model_row,
            &self.effort_row,
            &self.options,
            &self.efforts,
            keep,
        );
    }
}

fn selected_effort(effort_row: &adw::ComboRow, efforts: &RefCell<Vec<String>>) -> String {
    efforts
        .borrow()
        .get(effort_row.selected() as usize)
        .cloned()
        .unwrap_or_default()
}

fn select_effort(effort_row: &adw::ComboRow, efforts: &RefCell<Vec<String>>, value: &str) {
    let index = efforts.borrow().iter().position(|effort| effort == value);
    effort_row.set_selected(index.unwrap_or(0) as u32);
}

/// Offers the efforts the chosen model supports, keeping `keep` when it is one.
fn refresh_efforts(
    model_row: &adw::ComboRow,
    effort_row: &adw::ComboRow,
    options: &RefCell<Vec<ModelOption>>,
    efforts: &RefCell<Vec<String>>,
    keep: &str,
) {
    let supported = options
        .borrow()
        .get(model_row.selected() as usize)
        .map(|option| option.efforts.clone())
        .unwrap_or_default();
    effort_row.set_visible(!supported.is_empty());
    let mut values = vec![String::new()];
    values.extend(supported);
    let labels = values
        .iter()
        .map(|effort| {
            if effort.is_empty() {
                AGENT_DEFAULT
            } else {
                effort
            }
        })
        .collect::<Vec<_>>();
    effort_row.set_model(Some(&gtk::StringList::new(&labels)));
    *efforts.borrow_mut() = values;
    select_effort(effort_row, efforts, keep);
}

impl Workspace {
    /// Reads each agent's models off the main thread; Codex keeps them in a cache file.
    pub(super) fn load_model_options(
        &self,
        kinds: Vec<SessionKind>,
        done: impl FnOnce(&Self, AgentModels) + 'static,
    ) {
        self.run_io(
            move || {
                let codex_home = DiscoveryRoots::from_environment().codex_home_dir;
                Ok(kinds
                    .into_iter()
                    .map(|kind| (kind, agent_models::model_options(kind, &codex_home)))
                    .collect())
            },
            move |workspace, result| {
                if let Ok(options) = result {
                    done(workspace, options);
                }
            },
        );
    }
}
