//! Modal dialogs: binding wizard, layer manager, layout picker, confirmations,
//! unlock instructions and help.
//!
//! Key handling in this module is pure: it updates modal state and returns a
//! [`UiAction`] for the app to execute asynchronously.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::ListState;

use crate::keycodes;
use crate::labels;
use crate::model::{Behavior, Binding, Catalog, Keymap, ParamDesc, ParamKind, ParamSet};

#[derive(Debug, Clone)]
pub enum UiAction {
    Close,
    ApplyBinding {
        layer_id: u32,
        position: usize,
        binding: Binding,
    },
    AddLayer,
    AskRemoveLayer(usize),
    RemoveLayer(usize),
    MoveLayer {
        from: usize,
        to: usize,
    },
    RenameLayer {
        layer_id: u32,
        name: String,
    },
    SelectLayer(usize),
    SetLayout(usize),
    Save,
    Discard,
    ResetSettings,
    Quit,
    QuitSave,
    QuitDiscard,
    QuitAnyway,
    CheckUnlock,
}

pub enum Modal {
    Binding(BindingWizard),
    Layers(LayerModal),
    Layouts(LayoutModal),
    Unlock(UnlockModal),
    Confirm(ConfirmModal),
    Help,
}

impl Modal {
    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        keymap: &Keymap,
        catalog: &Catalog,
    ) -> Option<UiAction> {
        match self {
            Modal::Binding(wizard) => wizard.handle_key(key, keymap, catalog),
            Modal::Layers(layers) => layers.handle_key(key, keymap),
            Modal::Layouts(layouts) => layouts.handle_key(key),
            Modal::Unlock(unlock) => unlock.handle_key(key),
            Modal::Confirm(confirm) => confirm.handle_key(key),
            Modal::Help => {
                if matches!(
                    key.code,
                    KeyCode::Esc | KeyCode::Enter | KeyCode::Char('?') | KeyCode::Char('q')
                ) {
                    Some(UiAction::Close)
                } else {
                    None
                }
            }
        }
    }
}

// --------------------------------------------------------------------------
// Confirmation
// --------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ConfirmChoice {
    pub key: char,
    pub label: String,
    pub action: UiAction,
}

pub struct ConfirmModal {
    pub title: String,
    pub message: String,
    pub choices: Vec<ConfirmChoice>,
}

impl ConfirmModal {
    pub fn new(
        title: impl Into<String>,
        message: impl Into<String>,
        choices: Vec<ConfirmChoice>,
    ) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
            choices,
        }
    }

    pub fn quit() -> Self {
        Self::new(
            "Unsaved changes",
            "This keymap has edits that have not been saved to the keyboard.\n\
             Saving writes them to the keyboard's settings storage.",
            vec![
                ConfirmChoice {
                    key: 's',
                    label: "Save & quit".into(),
                    action: UiAction::QuitSave,
                },
                ConfirmChoice {
                    key: 'd',
                    label: "Discard & quit".into(),
                    action: UiAction::QuitDiscard,
                },
                ConfirmChoice {
                    key: 'q',
                    label: "Quit anyway".into(),
                    action: UiAction::QuitAnyway,
                },
            ],
        )
    }

    pub fn reset_settings() -> Self {
        Self::new(
            "Restore stock settings",
            "Restore the firmware's stock keymap? Every saved Studio change \
             (bindings, layers and physical layout selection) will be lost.",
            vec![
                ConfirmChoice {
                    key: 'y',
                    label: "Restore stock".into(),
                    action: UiAction::ResetSettings,
                },
                ConfirmChoice {
                    key: 'n',
                    label: "Cancel".into(),
                    action: UiAction::Close,
                },
            ],
        )
    }

    pub fn discard_changes() -> Self {
        Self::new(
            "Discard changes",
            "Revert every unsaved keymap change on the keyboard?",
            vec![
                ConfirmChoice {
                    key: 'y',
                    label: "Discard".into(),
                    action: UiAction::Discard,
                },
                ConfirmChoice {
                    key: 'n',
                    label: "Cancel".into(),
                    action: UiAction::Close,
                },
            ],
        )
    }

    pub fn remove_layer(index: usize, name: &str) -> Self {
        Self::new(
            "Remove layer",
            format!("Remove layer {index} “{name}”? The keyboard applies this immediately."),
            vec![
                ConfirmChoice {
                    key: 'y',
                    label: "Remove".into(),
                    action: UiAction::RemoveLayer(index),
                },
                ConfirmChoice {
                    key: 'n',
                    label: "Cancel".into(),
                    action: UiAction::Close,
                },
            ],
        )
    }

    fn handle_key(&self, key: KeyEvent) -> Option<UiAction> {
        match key.code {
            KeyCode::Esc => Some(UiAction::Close),
            KeyCode::Enter => self.choices.first().map(|choice| choice.action.clone()),
            KeyCode::Char(ch) => self
                .choices
                .iter()
                .find(|choice| choice.key.eq_ignore_ascii_case(&ch))
                .map(|choice| choice.action.clone()),
            _ => None,
        }
    }
}

// --------------------------------------------------------------------------
// Unlock
// --------------------------------------------------------------------------

#[derive(Default)]
pub struct UnlockModal {
    pub waiting: bool,
}

impl UnlockModal {
    fn handle_key(&self, key: KeyEvent) -> Option<UiAction> {
        match key.code {
            KeyCode::Esc => Some(UiAction::Close),
            KeyCode::Enter | KeyCode::Char('u') | KeyCode::Char('r') => Some(UiAction::CheckUnlock),
            _ => None,
        }
    }
}

// --------------------------------------------------------------------------
// Layer manager
// --------------------------------------------------------------------------

pub struct RenameState {
    pub layer_id: u32,
    pub value: String,
    pub max_len: usize,
}

pub struct LayerModal {
    pub state: ListState,
    pub renaming: Option<RenameState>,
}

impl LayerModal {
    pub fn new(selected_layer: usize, layer_count: usize) -> Self {
        let mut state = ListState::default();
        if layer_count > 0 {
            state.select(Some(selected_layer.min(layer_count - 1)));
        }
        Self {
            state,
            renaming: None,
        }
    }

    fn handle_key(&mut self, key: KeyEvent, keymap: &Keymap) -> Option<UiAction> {
        if self.renaming.is_some() {
            return self.handle_rename_key(key);
        }
        self.handle_layer_list_key(key, keymap)
    }

    fn handle_rename_key(&mut self, key: KeyEvent) -> Option<UiAction> {
        let Some(rename) = &mut self.renaming else {
            return None;
        };
        match key.code {
            KeyCode::Esc => {
                self.renaming = None;
            }
            KeyCode::Enter => {
                let trimmed = rename.value.trim().to_string();
                let layer_id = rename.layer_id;
                self.renaming = None;
                if trimmed.is_empty() {
                    return None;
                }
                return Some(UiAction::RenameLayer {
                    layer_id,
                    name: trimmed,
                });
            }
            KeyCode::Backspace => {
                rename.value.pop();
            }
            KeyCode::Char(ch)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    && rename.value.len() + ch.len_utf8() <= rename.max_len =>
            {
                rename.value.push(ch);
            }
            _ => {}
        }
        None
    }

    fn handle_layer_list_key(&mut self, key: KeyEvent, keymap: &Keymap) -> Option<UiAction> {
        let len = keymap.layers.len();
        match key.code {
            KeyCode::Esc => Some(UiAction::Close),
            KeyCode::Up => {
                move_selection(&mut self.state, len, -1);
                None
            }
            KeyCode::Down => {
                move_selection(&mut self.state, len, 1);
                None
            }
            KeyCode::Home => {
                first(&mut self.state, len);
                None
            }
            KeyCode::End => {
                last(&mut self.state, len);
                None
            }
            KeyCode::Enter => selected(&self.state, len).map(UiAction::SelectLayer),
            KeyCode::Char('a') => Some(UiAction::AddLayer),
            KeyCode::Char('x') => selected(&self.state, len).map(UiAction::AskRemoveLayer),
            KeyCode::Char('J') => {
                let index = selected(&self.state, len)?;
                if index + 1 < len {
                    Some(UiAction::MoveLayer {
                        from: index,
                        to: index + 1,
                    })
                } else {
                    None
                }
            }
            KeyCode::Char('K') => {
                let index = selected(&self.state, len)?;
                if index > 0 {
                    Some(UiAction::MoveLayer {
                        from: index,
                        to: index - 1,
                    })
                } else {
                    None
                }
            }
            KeyCode::Char('r') => {
                let index = selected(&self.state, len)?;
                let layer = keymap.layers.get(index)?;
                self.renaming = Some(RenameState {
                    layer_id: layer.id,
                    value: layer.name.clone(),
                    max_len: keymap.max_layer_name_length.clamp(4, 64),
                });
                None
            }
            _ => None,
        }
    }
}

// --------------------------------------------------------------------------
// Physical layout picker
// --------------------------------------------------------------------------

pub struct LayoutModal {
    pub state: ListState,
}

impl LayoutModal {
    pub fn new(active: usize, count: usize) -> Self {
        let mut state = ListState::default();
        if count > 0 {
            state.select(Some(active.min(count - 1)));
        }
        Self { state }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Option<UiAction> {
        // The layout count is not known here; the app clamps out-of-range
        // selections when it renders or applies them.
        match key.code {
            KeyCode::Esc => Some(UiAction::Close),
            KeyCode::Up => {
                if let Some(selected) = self.state.selected() {
                    self.state.select(Some(selected.saturating_sub(1)));
                }
                None
            }
            KeyCode::Down => {
                let selected = self.state.selected().unwrap_or(0);
                self.state.select(Some(selected + 1));
                None
            }
            KeyCode::Enter => self.state.selected().map(UiAction::SetLayout),
            _ => None,
        }
    }
}

// --------------------------------------------------------------------------
// Binding wizard
// --------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Behavior,
    Param1,
    Param2,
    Review,
}

pub struct BindingWizard {
    pub position: usize,
    pub layer_index: usize,
    pub stage: Stage,
    pub behavior: u32,
    pub behavior_filter: String,
    pub behavior_state: ListState,
    pub param1: u32,
    pub param2: u32,
    pub param_filter: String,
    pub param_state: ListState,
    pub range_value: i32,
    pub range_min: i32,
    pub range_max: i32,
}

#[derive(Debug, Clone)]
pub struct ParamItem {
    pub label: String,
    pub hint: String,
    pub value: u32,
}

impl BindingWizard {
    pub fn new(keymap: &Keymap, catalog: &Catalog, layer_index: usize, position: usize) -> Self {
        let current = keymap
            .layers
            .get(layer_index)
            .and_then(|layer| layer.bindings.get(position))
            .copied()
            .unwrap_or_default();

        let behaviors = behavior_items(catalog, "");
        let behavior = if catalog.get(current.behavior_id).is_some() {
            current.behavior_id
        } else {
            behaviors.first().map(|b| b.id).unwrap_or(0)
        };
        let selected = behaviors.iter().position(|b| b.id == behavior).unwrap_or(0);
        let mut behavior_state = ListState::default();
        if !behaviors.is_empty() {
            behavior_state.select(Some(selected));
        }

        Self {
            position,
            layer_index,
            stage: Stage::Behavior,
            behavior,
            behavior_filter: String::new(),
            behavior_state,
            param1: current.param1,
            param2: current.param2,
            param_filter: String::new(),
            param_state: ListState::default(),
            range_value: 0,
            range_min: 0,
            range_max: 0,
        }
    }

    pub fn current_binding(&self, keymap: &Keymap) -> Binding {
        keymap
            .layers
            .get(self.layer_index)
            .and_then(|layer| layer.bindings.get(self.position))
            .copied()
            .unwrap_or_default()
    }

    pub fn pending_binding(&self) -> Binding {
        Binding {
            behavior_id: self.behavior,
            param1: self.param1,
            param2: self.param2,
        }
    }

    fn handle_key(
        &mut self,
        key: KeyEvent,
        keymap: &Keymap,
        catalog: &Catalog,
    ) -> Option<UiAction> {
        match self.stage {
            Stage::Behavior => self.handle_behavior_key(key, keymap, catalog),
            Stage::Param1 | Stage::Param2 => self.handle_param_key(key, keymap, catalog),
            Stage::Review => match key.code {
                KeyCode::Esc => {
                    self.stage = Stage::Behavior;
                    self.reset_behavior_filter(catalog);
                    None
                }
                KeyCode::Enter => {
                    let layer_id = keymap.layers.get(self.layer_index)?.id;
                    Some(UiAction::ApplyBinding {
                        layer_id,
                        position: self.position,
                        binding: self.pending_binding(),
                    })
                }
                _ => None,
            },
        }
    }

    fn handle_behavior_key(
        &mut self,
        key: KeyEvent,
        keymap: &Keymap,
        catalog: &Catalog,
    ) -> Option<UiAction> {
        let items = behavior_items(catalog, &self.behavior_filter);
        if navigate_list(key.code, &mut self.behavior_state, items.len()) {
            return None;
        }
        match key.code {
            KeyCode::Esc => Some(UiAction::Close),
            KeyCode::Backspace => {
                self.behavior_filter.pop();
                reset_list(&mut self.behavior_state, items.len());
                None
            }
            KeyCode::Char(ch) if is_text_input(key, ch) => {
                self.behavior_filter.push(ch);
                reset_list(&mut self.behavior_state, items.len());
                None
            }
            KeyCode::Enter => {
                let index = selected(&self.behavior_state, items.len())?;
                let behavior = items[index];
                let changed = behavior.id != self.behavior;
                self.behavior = behavior.id;
                if changed {
                    self.param1 = 0;
                    self.param2 = 0;
                }
                self.enter_next_param_stage(keymap, catalog);
                None
            }
            _ => None,
        }
    }

    fn handle_param_key(
        &mut self,
        key: KeyEvent,
        keymap: &Keymap,
        catalog: &Catalog,
    ) -> Option<UiAction> {
        let stage = self.stage;
        let Some(desc) = param_desc(catalog, self.behavior, stage) else {
            self.advance_after_param(keymap, catalog);
            return None;
        };

        match desc.kind {
            ParamKind::Range { min, max } => {
                self.handle_range_param(key, keymap, catalog, min, max)
            }
            _ => self.handle_choice_param(key, keymap, catalog, stage),
        }
    }

    fn handle_range_param(
        &mut self,
        key: KeyEvent,
        keymap: &Keymap,
        catalog: &Catalog,
        min: i32,
        max: i32,
    ) -> Option<UiAction> {
        self.range_min = min;
        self.range_max = max;
        match key.code {
            KeyCode::Esc => {
                self.stage = Stage::Behavior;
                self.reset_behavior_filter(catalog);
            }
            KeyCode::Up => {
                self.range_value = (self.range_value + 1).min(max);
                self.set_stage_value(self.range_value.max(min) as u32);
            }
            KeyCode::Down => {
                self.range_value = (self.range_value - 1).max(min);
                self.set_stage_value(self.range_value.max(min) as u32);
            }
            KeyCode::Enter => {
                self.set_stage_value(self.range_value.max(min) as u32);
                self.advance_after_param(keymap, catalog);
            }
            KeyCode::Backspace => self.range_value /= 10,
            KeyCode::Char(ch) if is_text_input(key, ch) && ch.is_ascii_digit() => {
                let digit = ch.to_digit(10).unwrap_or(0) as i32;
                self.range_value = (self.range_value.saturating_mul(10) + digit).clamp(min, max);
                self.set_stage_value(self.range_value as u32);
            }
            _ => {}
        }
        None
    }

    fn handle_choice_param(
        &mut self,
        key: KeyEvent,
        keymap: &Keymap,
        catalog: &Catalog,
        stage: Stage,
    ) -> Option<UiAction> {
        let items = param_items(keymap, catalog, self.behavior, stage, &self.param_filter);
        if navigate_list(key.code, &mut self.param_state, items.len()) {
            return None;
        }
        match key.code {
            KeyCode::Esc => {
                self.stage = Stage::Behavior;
                self.reset_behavior_filter(catalog);
            }
            KeyCode::Backspace => {
                self.param_filter.pop();
                reset_list(&mut self.param_state, items.len());
            }
            KeyCode::Char(ch) if is_text_input(key, ch) => {
                self.param_filter.push(ch);
                reset_list(&mut self.param_state, items.len());
            }
            KeyCode::Enter => {
                let index = selected(&self.param_state, items.len())?;
                let value = items[index].value;
                self.set_stage_value(value);
                self.advance_after_param(keymap, catalog);
            }
            _ => {}
        }
        None
    }

    fn set_stage_value(&mut self, value: u32) {
        match self.stage {
            Stage::Param1 => self.param1 = value,
            Stage::Param2 => self.param2 = value,
            _ => {}
        }
    }

    fn advance_after_param(&mut self, keymap: &Keymap, catalog: &Catalog) {
        self.stage = match self.stage {
            Stage::Param1 => {
                if has_param(catalog, self.behavior, Stage::Param2) {
                    self.init_param_stage(keymap, catalog, Stage::Param2);
                    Stage::Param2
                } else {
                    Stage::Review
                }
            }
            Stage::Param2 | Stage::Review => Stage::Review,
            Stage::Behavior => Stage::Review,
        };
    }

    fn enter_next_param_stage(&mut self, keymap: &Keymap, catalog: &Catalog) {
        if has_param(catalog, self.behavior, Stage::Param1) {
            self.init_param_stage(keymap, catalog, Stage::Param1);
            self.stage = Stage::Param1;
        } else if has_param(catalog, self.behavior, Stage::Param2) {
            self.init_param_stage(keymap, catalog, Stage::Param2);
            self.stage = Stage::Param2;
        } else {
            self.stage = Stage::Review;
        }
    }

    fn init_param_stage(&mut self, keymap: &Keymap, catalog: &Catalog, stage: Stage) {
        self.param_filter.clear();
        let desc = param_desc(catalog, self.behavior, stage);
        if let Some(desc) = desc {
            if let ParamKind::Range { min, max } = desc.kind {
                self.range_min = min;
                self.range_max = max;
                let current = match stage {
                    Stage::Param1 => self.param1,
                    Stage::Param2 => self.param2,
                    _ => 0,
                };
                self.range_value = (current as i32).clamp(min, max);
                self.param_state.select(None);
                return;
            }
        }
        let items = param_items(keymap, catalog, self.behavior, stage, "");
        let wanted = match stage {
            Stage::Param1 => self.param1,
            Stage::Param2 => self.param2,
            _ => 0,
        };
        let index = items
            .iter()
            .position(|item| item.value == wanted)
            .unwrap_or(0);
        if items.is_empty() {
            self.param_state.select(None);
        } else {
            self.param_state.select(Some(index));
            *self.param_state.offset_mut() = 0;
        }
    }

    fn reset_behavior_filter(&mut self, catalog: &Catalog) {
        self.behavior_filter.clear();
        let items = behavior_items(catalog, "");
        let index = items
            .iter()
            .position(|behavior| behavior.id == self.behavior)
            .unwrap_or(0);
        if items.is_empty() {
            self.behavior_state.select(None);
        } else {
            self.behavior_state.select(Some(index));
            *self.behavior_state.offset_mut() = 0;
        }
    }
}

/// Apply the common list-navigation keys. Returns true when the key was used.
fn navigate_list(code: KeyCode, state: &mut ListState, len: usize) -> bool {
    match code {
        KeyCode::Up => move_selection(state, len, -1),
        KeyCode::Down => move_selection(state, len, 1),
        KeyCode::PageUp => move_selection(state, len, -10),
        KeyCode::PageDown => move_selection(state, len, 10),
        KeyCode::Home => first(state, len),
        KeyCode::End => last(state, len),
        _ => return false,
    }
    true
}

fn is_text_input(key: KeyEvent, ch: char) -> bool {
    !ch.is_control()
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
}

pub fn behavior_items<'a>(catalog: &'a Catalog, filter: &str) -> Vec<&'a Behavior> {
    let query = filter.trim().to_lowercase();
    catalog
        .sorted()
        .into_iter()
        .filter(|behavior| {
            query.is_empty() || behavior.display_name.to_lowercase().contains(&query)
        })
        .collect()
}

pub fn param_set(catalog: &Catalog, behavior_id: u32) -> Option<&ParamSet> {
    catalog.get(behavior_id)?.primary_param_set()
}

pub fn param_desc(catalog: &Catalog, behavior_id: u32, stage: Stage) -> Option<&ParamDesc> {
    let set = param_set(catalog, behavior_id)?;
    match stage {
        Stage::Param1 => set.param1.first(),
        Stage::Param2 => set.param2.first(),
        _ => None,
    }
}

pub fn has_param(catalog: &Catalog, behavior_id: u32, stage: Stage) -> bool {
    param_desc(catalog, behavior_id, stage)
        .map(|desc| !matches!(desc.kind, ParamKind::Nil))
        .unwrap_or(false)
}

pub fn param_items(
    keymap: &Keymap,
    catalog: &Catalog,
    behavior_id: u32,
    stage: Stage,
    filter: &str,
) -> Vec<ParamItem> {
    let Some(desc) = param_desc(catalog, behavior_id, stage) else {
        return Vec::new();
    };
    match desc.kind {
        ParamKind::LayerId => layer_param_items(keymap, filter),
        ParamKind::HidUsage { .. } => hid_usage_param_items(filter),
        ParamKind::Constant(_) => constant_param_items(catalog, behavior_id, stage, filter),
        ParamKind::Range { .. } | ParamKind::Nil => Vec::new(),
    }
}

fn layer_param_items(keymap: &Keymap, filter: &str) -> Vec<ParamItem> {
    let query = filter.trim().to_lowercase();
    keymap
        .layers
        .iter()
        .enumerate()
        .filter_map(|(index, layer)| {
            let name = layer.label(index);
            let searchable = format!("{index} {name}").to_lowercase();
            (query.is_empty() || searchable.contains(&query)).then(|| ParamItem {
                label: format!("{index:>2}  {name}"),
                hint: format!("layer id {}", layer.id),
                value: layer.id,
            })
        })
        .collect()
}

fn hid_usage_param_items(filter: &str) -> Vec<ParamItem> {
    keycodes::search(filter)
        .into_iter()
        .map(|keycode| {
            let mut hint = String::new();
            if keycode.name != keycode.label {
                hint.push_str(keycode.name);
                hint.push_str("  ·  ");
            }
            hint.push_str(keycode.category.label());
            ParamItem {
                label: keycode.label.to_string(),
                hint,
                value: keycode.value,
            }
        })
        .collect()
}

fn constant_param_items(
    catalog: &Catalog,
    behavior_id: u32,
    stage: Stage,
    filter: &str,
) -> Vec<ParamItem> {
    let query = filter.trim().to_lowercase();
    let values = param_set(catalog, behavior_id)
        .map(|set| match stage {
            Stage::Param1 => set.param1.as_slice(),
            Stage::Param2 => set.param2.as_slice(),
            _ => &[],
        })
        .unwrap_or(&[]);
    values
        .iter()
        .filter_map(|value| match value.kind {
            ParamKind::Constant(constant)
                if query.is_empty() || value.name.to_lowercase().contains(&query) =>
            {
                Some(ParamItem {
                    label: value.name.clone(),
                    hint: format!("0x{constant:X}"),
                    value: constant,
                })
            }
            _ => None,
        })
        .collect()
}

pub fn selected(state: &ListState, len: usize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some(state.selected().unwrap_or(0).min(len - 1))
}

pub fn move_selection(state: &mut ListState, len: usize, delta: isize) {
    if len == 0 {
        state.select(None);
        return;
    }
    let current = state.selected().unwrap_or(0).min(len - 1) as isize;
    let next = (current + delta).clamp(0, len as isize - 1) as usize;
    state.select(Some(next));
}

pub fn first(state: &mut ListState, len: usize) {
    if len > 0 {
        state.select(Some(0));
        *state.offset_mut() = 0;
    }
}

pub fn last(state: &mut ListState, len: usize) {
    if len > 0 {
        state.select(Some(len - 1));
    }
}

pub fn reset_list(state: &mut ListState, len: usize) {
    if len > 0 {
        state.select(Some(0));
        *state.offset_mut() = 0;
    } else {
        state.select(None);
    }
}

/// Full description of the selected binding for the wizard header.
pub fn binding_detail(keymap: &Keymap, catalog: &Catalog, binding: Binding) -> String {
    labels::binding_label(catalog, keymap, binding).detail
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Behavior, Binding, Catalog, Keymap, Layer, ParamDesc, ParamSet};

    const KEY_PRESS: u32 = 1;
    const MOD_TAP: u32 = 2;
    const MOMENTARY: u32 = 3;
    const RANGE: u32 = 4;
    const CONSTANT: u32 = 5;
    const PLAIN: u32 = 6;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn desc(name: &str, kind: ParamKind) -> ParamDesc {
        ParamDesc {
            name: name.into(),
            kind,
        }
    }

    fn catalog() -> Catalog {
        let mut catalog = Catalog::default();
        let hid = ParamKind::HidUsage {
            keyboard_max: 255,
            consumer_max: 1023,
        };
        for (id, name, set) in [
            (
                KEY_PRESS,
                "Key Press",
                ParamSet {
                    param1: vec![desc("Key", hid)],
                    param2: vec![],
                },
            ),
            (
                MOD_TAP,
                "Mod-Tap",
                ParamSet {
                    param1: vec![desc("Modifier", hid)],
                    param2: vec![desc("Key", hid)],
                },
            ),
            (
                MOMENTARY,
                "Momentary Layer",
                ParamSet {
                    param1: vec![desc("Layer", ParamKind::LayerId)],
                    param2: vec![],
                },
            ),
            (
                RANGE,
                "Custom Range",
                ParamSet {
                    param1: vec![desc("Amount", ParamKind::Range { min: 0, max: 10 })],
                    param2: vec![],
                },
            ),
            (
                CONSTANT,
                "Custom Constant",
                ParamSet {
                    param1: vec![
                        desc("First", ParamKind::Constant(0x10)),
                        desc("Second", ParamKind::Constant(0x20)),
                    ],
                    param2: vec![],
                },
            ),
        ] {
            catalog.insert(Behavior {
                id,
                display_name: name.into(),
                metadata: vec![set],
            });
        }
        catalog.insert(Behavior {
            id: PLAIN,
            display_name: "Custom Plain".into(),
            metadata: vec![],
        });
        catalog
    }

    fn keymap() -> Keymap {
        Keymap {
            layers: vec![
                Layer {
                    id: 0,
                    name: "Base".into(),
                    bindings: vec![Binding::default()],
                },
                Layer {
                    id: 7,
                    name: "Lower".into(),
                    bindings: vec![Binding::default()],
                },
            ],
            available_layers: 6,
            max_layer_name_length: 32,
        }
    }

    fn wizard() -> BindingWizard {
        BindingWizard::new(&keymap(), &catalog(), 0, 0)
    }

    #[test]
    fn param_items_lists_layers_keycodes_and_constants() {
        let keymap = keymap();
        let catalog = catalog();

        let layers = param_items(&keymap, &catalog, MOMENTARY, Stage::Param1, "low");
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].value, 7);
        assert!(layers[0].label.contains("Lower"));

        let keys = param_items(&keymap, &catalog, KEY_PRESS, Stage::Param1, "a");
        assert!(!keys.is_empty());
        assert!(keys.iter().any(|item| item.value == 0x0007_0004));

        let constants = param_items(&keymap, &catalog, CONSTANT, Stage::Param1, "");
        assert_eq!(constants.len(), 2);
        assert_eq!(constants[0].value, 0x10);
        assert_eq!(constants[1].label, "Second");
        assert_eq!(constants[1].hint, "0x20");
        assert_eq!(
            param_items(&keymap, &catalog, CONSTANT, Stage::Param1, "sec")[0].value,
            0x20
        );

        assert!(param_items(&keymap, &catalog, RANGE, Stage::Param1, "").is_empty());
        assert!(param_items(&keymap, &catalog, PLAIN, Stage::Param1, "").is_empty());
    }

    #[test]
    fn range_param_clamps_digits_and_advances_to_review() {
        let catalog = catalog();
        let keymap = keymap();
        let mut wizard = wizard();
        wizard.behavior = RANGE;
        wizard.stage = Stage::Param1;
        wizard.range_value = 5;

        assert!(
            wizard
                .handle_key(key(KeyCode::Up), &keymap, &catalog)
                .is_none()
        );
        assert_eq!(wizard.range_value, 6);
        wizard.handle_key(key(KeyCode::Char('9')), &keymap, &catalog);
        assert_eq!(wizard.range_value, 10, "digits clamp to max");
        wizard.handle_key(key(KeyCode::Char('9')), &keymap, &catalog);
        assert_eq!(wizard.range_value, 10);
        wizard.handle_key(key(KeyCode::Backspace), &keymap, &catalog);
        assert_eq!(wizard.range_value, 1);
        wizard.handle_key(key(KeyCode::Down), &keymap, &catalog);
        assert_eq!(wizard.range_value, 0, "down clamps to min");
        wizard.handle_key(key(KeyCode::Enter), &keymap, &catalog);
        assert_eq!(wizard.param1, 0);
        assert_eq!(wizard.stage, Stage::Review);
    }

    #[test]
    fn choice_param_selects_a_layer_and_advances() {
        let catalog = catalog();
        let keymap = keymap();
        let mut wizard = wizard();
        wizard.behavior = MOMENTARY;
        wizard.stage = Stage::Param1;
        wizard.param_state.select(Some(1));

        wizard.handle_key(key(KeyCode::Enter), &keymap, &catalog);
        assert_eq!(wizard.param1, 7);
        assert_eq!(wizard.stage, Stage::Review);
    }

    #[test]
    fn behavior_filter_and_navigation_select_mod_tap() {
        let catalog = catalog();
        let keymap = keymap();
        let mut wizard = wizard();

        for ch in "mod".chars() {
            wizard.handle_key(key(KeyCode::Char(ch)), &keymap, &catalog);
        }
        assert_eq!(wizard.behavior_filter, "mod");
        let items = behavior_items(&catalog, &wizard.behavior_filter);
        assert!(!items.is_empty());
        wizard.handle_key(key(KeyCode::PageDown), &keymap, &catalog);
        wizard.handle_key(key(KeyCode::Home), &keymap, &catalog);
        wizard.handle_key(key(KeyCode::Enter), &keymap, &catalog);
        assert_eq!(wizard.behavior, MOD_TAP);
        assert_eq!(wizard.stage, Stage::Param1);
    }

    #[test]
    fn range_escape_returns_to_behavior_stage() {
        let catalog = catalog();
        let keymap = keymap();
        let mut wizard = wizard();
        wizard.behavior = RANGE;
        wizard.stage = Stage::Param1;
        wizard.handle_key(key(KeyCode::Esc), &keymap, &catalog);
        assert_eq!(wizard.stage, Stage::Behavior);
    }

    #[test]
    fn layer_modal_navigates_and_emits_actions() {
        let keymap = keymap();
        let mut modal = LayerModal::new(0, keymap.layers.len());
        assert!(matches!(
            modal.handle_key(key(KeyCode::Enter), &keymap),
            Some(UiAction::SelectLayer(0))
        ));
        modal.handle_key(key(KeyCode::Down), &keymap);
        assert!(matches!(
            modal.handle_key(key(KeyCode::Enter), &keymap),
            Some(UiAction::SelectLayer(1))
        ));
        assert!(matches!(
            modal.handle_key(key(KeyCode::Char('x')), &keymap),
            Some(UiAction::AskRemoveLayer(1))
        ));
        assert!(matches!(
            modal.handle_key(key(KeyCode::Char('K')), &keymap),
            Some(UiAction::MoveLayer { from: 1, to: 0 })
        ));
        modal.handle_key(key(KeyCode::Home), &keymap);
        assert!(matches!(
            modal.handle_key(key(KeyCode::Char('J')), &keymap),
            Some(UiAction::MoveLayer { from: 0, to: 1 })
        ));
        assert!(matches!(
            modal.handle_key(key(KeyCode::Char('a')), &keymap),
            Some(UiAction::AddLayer)
        ));
        assert!(matches!(
            modal.handle_key(key(KeyCode::Esc), &keymap),
            Some(UiAction::Close)
        ));
    }

    #[test]
    fn layer_rename_flow_edits_and_commits() {
        let keymap = keymap();
        let mut modal = LayerModal::new(0, keymap.layers.len());
        assert!(modal.handle_key(key(KeyCode::Char('r')), &keymap).is_none());
        let rename = modal.renaming.as_mut().expect("rename mode");
        rename.value.clear();
        for ch in "Nav".chars() {
            modal.handle_key(key(KeyCode::Char(ch)), &keymap);
        }
        assert_eq!(modal.renaming.as_ref().unwrap().value, "Nav");
        let action = modal.handle_key(key(KeyCode::Enter), &keymap);
        assert!(matches!(
            action,
            Some(UiAction::RenameLayer { layer_id: 0, name }) if name == "Nav"
        ));
        assert!(modal.renaming.is_none());
    }

    #[test]
    fn layout_modal_navigation_and_selection() {
        let mut modal = LayoutModal::new(1, 3);
        assert_eq!(modal.state.selected(), Some(1));
        modal.handle_key(key(KeyCode::Down));
        assert_eq!(modal.state.selected(), Some(2));
        modal.handle_key(key(KeyCode::Up));
        assert!(matches!(
            modal.handle_key(key(KeyCode::Enter)),
            Some(UiAction::SetLayout(1))
        ));
        assert!(matches!(
            modal.handle_key(key(KeyCode::Esc)),
            Some(UiAction::Close)
        ));
    }

    #[test]
    fn list_helpers_clamp_and_reset() {
        let mut state = ListState::default();
        assert_eq!(selected(&state, 0), None);
        move_selection(&mut state, 3, 1);
        assert_eq!(state.selected(), Some(1));
        move_selection(&mut state, 3, -10);
        assert_eq!(state.selected(), Some(0));
        move_selection(&mut state, 3, 10);
        assert_eq!(state.selected(), Some(2));
        last(&mut state, 3);
        assert_eq!(state.selected(), Some(2));
        first(&mut state, 3);
        assert_eq!(state.selected(), Some(0));
        reset_list(&mut state, 0);
        assert_eq!(state.selected(), None);
        assert!(navigate_list(KeyCode::Down, &mut state, 2));
        assert!(!navigate_list(KeyCode::Char('x'), &mut state, 2));
    }

    #[test]
    fn layer_rename_can_be_cancelled_or_left_empty() {
        let keymap = keymap();
        let mut modal = LayerModal::new(0, keymap.layers.len());
        modal.handle_key(key(KeyCode::Char('r')), &keymap);
        assert!(modal.renaming.is_some());
        modal.handle_key(key(KeyCode::Esc), &keymap);
        assert!(modal.renaming.is_none());

        modal.handle_key(key(KeyCode::Char('r')), &keymap);
        modal.renaming.as_mut().unwrap().value.clear();
        assert!(modal.handle_key(key(KeyCode::Enter), &keymap).is_none());
        assert!(modal.renaming.is_none());
    }

    #[test]
    fn layer_list_is_safe_with_no_layers() {
        let empty = Keymap::default();
        let mut modal = LayerModal::new(0, 0);
        for code in [
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Home,
            KeyCode::End,
            KeyCode::Enter,
            KeyCode::Char('x'),
            KeyCode::Char('J'),
            KeyCode::Char('K'),
            KeyCode::Char('r'),
        ] {
            assert!(modal.handle_key(key(code), &empty).is_none());
        }
    }
}
