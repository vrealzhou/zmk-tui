//! Application state, key handling and asynchronous actions.

use std::time::{Duration, Instant};

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::widgets::ListState;
use tokio::sync::mpsc::UnboundedSender;

use crate::client::{StudioError, StudioEvent};
use crate::device::{Capabilities, DeviceClient};
use crate::layout::{self, Direction};
use crate::model::{
    Binding, Catalog, DeviceInfo, KeyGeometry, Keymap, LockState, PhysicalLayout, PhysicalLayouts,
};
use crate::simulator::{SimulatedBoard, Simulator};
use crate::transport::{ConnectionKind, ble, serial};
use crate::tui::connection::{
    Connected, DeviceChoice, Snapshot, UsbEntry, connect_by_bluetooth_name, connect_choice,
    finish_connection, list_usb_entries, load_snapshot, scan_bluetooth,
};
use crate::tui::modals::{
    BindingWizard, ConfirmModal, LayerModal, LayoutModal, Modal, UiAction, UnlockModal,
};
use crate::tui::mouse::{ListKind, MouseMap, MouseTarget};

/// Delay before retrying a failed keymap read from the keyboard.
const SNAPSHOT_RETRY_DELAY: Duration = Duration::from_secs(1);
/// How many times a failed keymap read is retried before giving up.
const MAX_SNAPSHOT_RETRIES: u32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VialStart {
    /// In-process Vial demo keyboard.
    Simulate,
    /// First USB raw-HID Vial interface.
    Usb,
}

#[derive(Debug, Clone, Default)]
pub struct StartOptions {
    pub port: Option<String>,
    pub ble: Option<String>,
    pub simulate: Option<SimulatedBoard>,
    pub vial: Option<VialStart>,
    /// Whether the connect screen also enumerates Vial raw-HID interfaces.
    /// The binary enables this; library tests keep it off so they never probe
    /// host hardware.
    pub scan_vial: bool,
    /// Opt-in (`--auto`): connect immediately when exactly one keyboard is
    /// visible. The default stays on the device list so the user can wait for
    /// a Bluetooth scan and choose between USB/HID and Bluetooth devices.
    pub auto_connect: bool,
}

/// What to do after a save/discard completes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterSave {
    Stay,
    Quit,
    Devices,
}

pub enum Msg {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Tick,
    /// The terminal input thread stopped (stdin closed or a read error).
    InputClosed,
    UsbDevices(Vec<UsbEntry>),
    BluetoothDevices(Result<Vec<ble::BleDevice>, String>),
    ConnectResult(Result<Connected, String>),
    DeviceEvent(StudioEvent),
    Snapshot(Result<Snapshot, StudioError>),
    BindingSet {
        result: Result<(), StudioError>,
        layer_id: u32,
        position: usize,
        binding: Binding,
    },
    Saved {
        result: Result<(), StudioError>,
        then: AfterSave,
    },
    Discarded {
        result: Result<(), StudioError>,
        then: AfterSave,
    },
    KeymapRefreshed(Result<Keymap, StudioError>),
    /// A keymap and physical-layout refresh, used after a discard because the
    /// firmware also reverts its physical-layout selection.
    EditRefreshed(Result<(Keymap, PhysicalLayouts), StudioError>),
    LayerAdded(Result<u32, StudioError>),
    LayerRemoved {
        result: Result<(), StudioError>,
        index: usize,
    },
    LayerMoved {
        result: Result<Keymap, StudioError>,
        to: usize,
    },
    LayerRenamed {
        result: Result<(), StudioError>,
        layer_id: u32,
        name: String,
    },
    LayoutChanged {
        result: Result<Keymap, StudioError>,
        index: usize,
    },
    LockPolled(Result<LockState, StudioError>),
    ResetSettings(Result<(), StudioError>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Usb,
    Bluetooth,
}

pub struct ConnectState {
    pub usb: Vec<UsbEntry>,
    pub bluetooth: Vec<ble::BleDevice>,
    pub focus: Panel,
    pub usb_state: ListState,
    pub bluetooth_state: ListState,
    pub scanning: bool,
    pub scan_error: Option<String>,
}

impl Default for ConnectState {
    fn default() -> Self {
        Self {
            usb: Vec::new(),
            bluetooth: Vec::new(),
            focus: Panel::Usb,
            usb_state: ListState::default(),
            bluetooth_state: ListState::default(),
            scanning: false,
            scan_error: None,
        }
    }
}

pub struct EditorState {
    pub client: DeviceClient,
    pub capabilities: Capabilities,
    pub label: String,
    pub kind: ConnectionKind,
    pub info: DeviceInfo,
    pub keymap: Keymap,
    pub layouts: PhysicalLayouts,
    pub catalog: Catalog,
    pub layer_index: usize,
    pub selected: usize,
    pub dirty: bool,
    pub locked: bool,
    pub keymap_loaded: bool,
}

impl EditorState {
    fn new(connected: Connected) -> Self {
        let has_snapshot = connected.snapshot.is_some();
        let snapshot = connected.snapshot.unwrap_or_default();
        let locked = connected.lock.is_locked();
        let capabilities = connected.client.capabilities();
        let mut editor = Self {
            client: connected.client,
            capabilities,
            label: connected.label,
            kind: connected.kind,
            info: connected.info,
            keymap: snapshot.keymap,
            layouts: snapshot.layouts,
            catalog: snapshot.catalog,
            layer_index: 0,
            selected: 0,
            dirty: snapshot.dirty,
            locked,
            keymap_loaded: has_snapshot,
        };
        editor.clamp();
        editor
    }

    pub fn layer(&self) -> Option<&crate::model::Layer> {
        self.keymap.layers.get(self.layer_index)
    }

    pub fn layer_name(&self) -> String {
        self.layer()
            .map(|layer| layer.label(self.layer_index))
            .unwrap_or_else(|| "—".into())
    }

    pub fn position_count(&self) -> usize {
        if let Some(layout) = self.layouts.active_or_first() {
            if !layout.keys.is_empty() {
                return layout.keys.len();
            }
        }
        self.keymap
            .layers
            .iter()
            .map(|layer| layer.bindings.len())
            .max()
            .unwrap_or(0)
    }

    /// The active physical layout, or a plain fallback grid when firmware
    /// reports no layout information.
    pub fn effective_layout(&self) -> PhysicalLayout {
        if let Some(layout) = self.layouts.active_or_first() {
            if !layout.keys.is_empty() {
                return layout.clone();
            }
        }
        fallback_layout(self.position_count())
    }

    /// Display name of the physical layout reported by the device.
    pub fn active_layout_name(&self) -> &str {
        self.layouts
            .active_or_first()
            .map(|layout| layout.name.as_str())
            .unwrap_or("generic grid")
    }

    pub fn selected_binding(&self) -> Option<Binding> {
        self.layer()
            .and_then(|layer| layer.bindings.get(self.selected))
            .copied()
    }

    pub fn clamp(&mut self) {
        let layer_count = self.keymap.layers.len();
        if layer_count == 0 {
            self.layer_index = 0;
        } else {
            self.layer_index = self.layer_index.min(layer_count - 1);
        }
        let positions = self.position_count().max(1);
        self.selected = self.selected.min(positions - 1);
    }
}

fn fallback_layout(count: usize) -> PhysicalLayout {
    let per_row = 10;
    PhysicalLayout {
        name: "Key positions".into(),
        keys: (0..count)
            .map(|index| KeyGeometry {
                x: (index % per_row) as f32,
                y: (index / per_row) as f32,
                width: 1.0,
                height: 1.0,
                ..Default::default()
            })
            .collect(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    Info,
    Success,
    Warning,
    Error,
}

pub struct Status {
    pub text: String,
    pub kind: StatusKind,
    pub at: Instant,
}

pub struct App {
    pub tx: UnboundedSender<Msg>,
    pub editor: Option<EditorState>,
    pub connect: ConnectState,
    pub modal: Option<Modal>,
    pub status: Option<Status>,
    pub busy: Option<String>,
    pub tick: u64,
    pub should_quit: bool,
    pub mouse: MouseMap,
    /// Keyboard-viewport size recorded by the last render, used so directional
    /// navigation follows the grid the user actually sees.
    pub keyboard_viewport: layout::Viewport,
    last_click: Option<(MouseTarget, Instant)>,
    pub start: StartOptions,
    auto_connect: bool,
    simulator: Option<Simulator>,
    connect_inflight: bool,
    scanning_inflight: bool,
    lock_poll_inflight: bool,
    last_lock_poll: Instant,
    snapshot_retry_at: Option<Instant>,
    pending_layer_select: Option<usize>,
    /// Incremented whenever a local keymap mutation starts. A snapshot that was
    /// requested before the mutation carries stale data and must not overwrite
    /// the newer local edits.
    mutation_generation: u64,
    snapshot_generation: u64,
    snapshot_attempts: u32,
}

impl App {
    pub fn new(tx: UnboundedSender<Msg>, start: StartOptions) -> Self {
        Self {
            tx,
            editor: None,
            connect: ConnectState::default(),
            modal: None,
            status: None,
            busy: None,
            tick: 0,
            should_quit: false,
            mouse: MouseMap::default(),
            keyboard_viewport: layout::Viewport::default(),
            last_click: None,
            auto_connect: start.auto_connect,
            start,
            simulator: None,
            connect_inflight: false,
            lock_poll_inflight: false,
            scanning_inflight: false,
            last_lock_poll: Instant::now(),
            snapshot_retry_at: None,
            pending_layer_select: None,
            mutation_generation: 0,
            snapshot_generation: 0,
            snapshot_attempts: 0,
        }
    }

    /// Record that a local keymap mutation is in flight: the keymap is now
    /// unsaved (even before the device confirms) and any snapshot requested
    /// earlier becomes stale.
    fn begin_mutation(&mut self) {
        self.mutation_generation = self.mutation_generation.wrapping_add(1);
        if let Some(editor) = &mut self.editor {
            editor.dirty = true;
        }
    }

    pub fn on_start(&mut self) {
        if let Some(vial) = self.start.vial {
            self.connect_inflight = true;
            self.busy = Some("Connecting to Vial keyboard…".into());
            let tx = self.tx.clone();
            tokio::spawn(async move {
                let result = crate::tui::connection::connect_vial(vial).await;
                let _ = tx.send(Msg::ConnectResult(result));
            });
            return;
        }
        if let Some(board) = self.start.simulate {
            self.start_simulator(board);
            return;
        }
        if let Some(port) = self.start.port.clone() {
            self.spawn_connect(DeviceChoice::Usb {
                path: port.clone(),
                label: port,
            });
            return;
        }
        if let Some(name) = self.start.ble.clone() {
            self.spawn_bluetooth_by_name(name);
            return;
        }
        self.refresh_usb();
        if self.try_auto_connect() {
            return;
        }
        self.spawn_bluetooth_scan();
    }

    pub fn handle(&mut self, message: Msg) {
        match message {
            Msg::Key(key) => self.handle_key(key),
            Msg::Mouse(mouse) => self.handle_mouse(mouse),
            Msg::Tick => self.on_tick(),
            Msg::InputClosed => self.should_quit = true,
            message @ (Msg::UsbDevices(_)
            | Msg::BluetoothDevices(_)
            | Msg::ConnectResult(_)
            | Msg::DeviceEvent(_)) => self.handle_connection_message(message),
            message @ (Msg::Snapshot(_)
            | Msg::BindingSet { .. }
            | Msg::Saved { .. }
            | Msg::Discarded { .. }
            | Msg::KeymapRefreshed(_)
            | Msg::EditRefreshed(_)) => self.handle_edit_message(message),
            message @ (Msg::LayerAdded(_)
            | Msg::LayerRemoved { .. }
            | Msg::LayerMoved { .. }
            | Msg::LayerRenamed { .. }
            | Msg::LayoutChanged { .. }
            | Msg::LockPolled(_)
            | Msg::ResetSettings(_)) => self.handle_layer_message(message),
        }
    }

    fn handle_connection_message(&mut self, message: Msg) {
        match message {
            Msg::UsbDevices(devices) => {
                self.connect.usb = devices;
                self.clamp_connect_selection();
            }
            Msg::BluetoothDevices(result) => {
                self.scanning_inflight = false;
                self.connect.scanning = false;
                match result {
                    Ok(devices) => {
                        self.connect.bluetooth = devices;
                        self.connect.scan_error = None;
                        self.clamp_connect_selection();
                        self.try_auto_connect();
                    }
                    Err(error) => {
                        self.connect.bluetooth.clear();
                        self.connect.scan_error = Some(error.clone());
                        self.toast(error, StatusKind::Warning);
                    }
                }
            }
            Msg::ConnectResult(result) => self.handle_connect_result(result),
            Msg::DeviceEvent(event) => self.handle_device_event(event),
            _ => unreachable!("only connection messages reach this handler"),
        }
    }

    fn handle_edit_message(&mut self, message: Msg) {
        // A completion that arrives after the keyboard disconnected must not
        // touch (or toast about) the now-empty editor.
        if self.editor.is_none() {
            return;
        }
        match message {
            Msg::Snapshot(result) => self.handle_snapshot(result),
            Msg::BindingSet {
                result,
                layer_id,
                position,
                binding,
            } => self.finish_binding_set(result, layer_id, position, binding),
            Msg::Saved { result, then } => self.finish_saved(result, then),
            Msg::Discarded { result, then } => self.finish_discarded(result, then),
            Msg::KeymapRefreshed(result) => self.finish_keymap_refresh(result),
            Msg::EditRefreshed(result) => self.finish_edit_refresh(result),
            _ => unreachable!("only edit messages reach this handler"),
        }
    }

    fn finish_binding_set(
        &mut self,
        result: Result<(), StudioError>,
        layer_id: u32,
        position: usize,
        binding: Binding,
    ) {
        if let Err(error) = result {
            self.handle_studio_error(error);
            return;
        }
        if let Some(editor) = &mut self.editor {
            if let Some(layer) = editor
                .keymap
                .layers
                .iter_mut()
                .find(|layer| layer.id == layer_id)
            {
                if let Some(slot) = layer.bindings.get_mut(position) {
                    *slot = binding;
                }
            }
            editor.dirty = true;
        }
        self.busy = None;
        self.toast("Binding updated — press s to save", StatusKind::Success);
    }

    fn finish_saved(&mut self, result: Result<(), StudioError>, then: AfterSave) {
        if let Err(error) = result {
            self.handle_studio_error(error);
            return;
        }
        if let Some(editor) = &mut self.editor {
            editor.dirty = false;
        }
        self.busy = None;
        match then {
            AfterSave::Stay => self.toast("Saved to keyboard", StatusKind::Success),
            AfterSave::Quit => {
                self.toast("Saved to keyboard", StatusKind::Success);
                self.should_quit = true;
            }
            AfterSave::Devices => {
                self.leave_editor();
                self.toast("Saved — choose a keyboard", StatusKind::Success);
            }
        }
    }

    fn finish_discarded(&mut self, result: Result<(), StudioError>, then: AfterSave) {
        if let Err(error) = result {
            self.handle_studio_error(error);
            return;
        }
        if let Some(editor) = &mut self.editor {
            editor.dirty = false;
        }
        self.busy = None;
        match then {
            AfterSave::Stay => {
                self.toast("Discarded unsaved changes", StatusKind::Info);
                self.spawn_refresh_edit();
            }
            AfterSave::Quit => {
                self.toast("Discarded unsaved changes", StatusKind::Info);
                self.should_quit = true;
            }
            AfterSave::Devices => {
                self.leave_editor();
                self.toast("Discarded — choose a keyboard", StatusKind::Info);
            }
        }
    }

    fn finish_keymap_refresh(&mut self, result: Result<Keymap, StudioError>) {
        let keymap = match result {
            Ok(keymap) => keymap,
            Err(error) => {
                self.pending_layer_select = None;
                self.handle_studio_error(error);
                return;
            }
        };
        if let Some(editor) = &mut self.editor {
            editor.keymap = keymap;
            editor.keymap_loaded = true;
            if let Some(index) = self.pending_layer_select.take() {
                editor.layer_index = index;
            }
            editor.clamp();
        }
        self.busy = None;
    }

    fn finish_edit_refresh(&mut self, result: Result<(Keymap, PhysicalLayouts), StudioError>) {
        let (keymap, layouts) = match result {
            Ok(refreshed) => refreshed,
            Err(error) => {
                self.pending_layer_select = None;
                self.handle_studio_error(error);
                return;
            }
        };
        if let Some(editor) = &mut self.editor {
            editor.keymap = keymap;
            editor.layouts = layouts;
            editor.keymap_loaded = true;
            if let Some(index) = self.pending_layer_select.take() {
                editor.layer_index = index;
            }
            editor.clamp();
        }
        self.busy = None;
    }

    fn handle_layer_message(&mut self, message: Msg) {
        if self.editor.is_none() {
            return;
        }
        match message {
            Msg::LayerAdded(result) => self.finish_layer_added(result),
            Msg::LayerRemoved { result, index } => self.finish_layer_removed(result, index),
            Msg::LayerMoved { result, to } => self.finish_layer_moved(result, to),
            Msg::LayerRenamed {
                result,
                layer_id,
                name,
            } => self.finish_layer_renamed(result, layer_id, name),
            Msg::LayoutChanged { result, index } => self.finish_layout_changed(result, index),
            Msg::LockPolled(result) => self.handle_lock_polled(result),
            Msg::ResetSettings(result) => self.finish_reset_settings(result),
            _ => unreachable!("only layer/session messages reach this handler"),
        }
    }

    fn finish_layer_added(&mut self, result: Result<u32, StudioError>) {
        match result {
            Ok(index) => {
                self.pending_layer_select = Some(index as usize);
                self.spawn_refresh_keymap();
                self.toast("Layer added", StatusKind::Success);
            }
            Err(error) => self.handle_studio_error(error),
        }
    }

    fn finish_layer_removed(&mut self, result: Result<(), StudioError>, index: usize) {
        if let Err(error) = result {
            self.handle_studio_error(error);
            return;
        }
        if let Some(editor) = &mut self.editor {
            // Keep showing the same layer when a layer *below* the selection
            // was removed; `clamp` handles removing the selected layer itself.
            if index < editor.layer_index {
                editor.layer_index -= 1;
            }
        }
        self.spawn_refresh_keymap();
        self.toast("Layer removed", StatusKind::Success);
    }

    fn finish_layer_moved(&mut self, result: Result<Keymap, StudioError>, to: usize) {
        let keymap = match result {
            Ok(keymap) => keymap,
            Err(error) => {
                self.handle_studio_error(error);
                return;
            }
        };
        if let Some(editor) = &mut self.editor {
            editor.keymap = keymap;
            editor.layer_index = to;
            editor.dirty = true;
            editor.clamp();
        }
        self.busy = None;
        self.toast("Layer moved", StatusKind::Success);
    }

    fn finish_layer_renamed(
        &mut self,
        result: Result<(), StudioError>,
        layer_id: u32,
        name: String,
    ) {
        if let Err(error) = result {
            self.handle_studio_error(error);
            return;
        }
        if let Some(editor) = &mut self.editor {
            if let Some(layer) = editor
                .keymap
                .layers
                .iter_mut()
                .find(|layer| layer.id == layer_id)
            {
                layer.name = name;
            }
            editor.dirty = true;
        }
        self.busy = None;
        self.toast("Layer renamed", StatusKind::Success);
    }

    fn finish_layout_changed(&mut self, result: Result<Keymap, StudioError>, index: usize) {
        let keymap = match result {
            Ok(keymap) => keymap,
            Err(error) => {
                self.handle_studio_error(error);
                return;
            }
        };
        if let Some(editor) = &mut self.editor {
            let changed = editor.layouts.active_index != index;
            editor.keymap = keymap;
            editor.layouts.active_index = index;
            if changed {
                editor.dirty = true;
            }
            editor.clamp();
        }
        self.busy = None;
        self.toast("Physical layout changed", StatusKind::Success);
    }

    fn finish_reset_settings(&mut self, result: Result<(), StudioError>) {
        if let Err(error) = result {
            self.handle_studio_error(error);
            return;
        }
        if let Some(editor) = &mut self.editor {
            editor.dirty = false;
        }
        self.busy = None;
        self.toast("Stock settings restored", StatusKind::Success);
        self.spawn_snapshot_load();
    }

    // ------------------------------------------------------------------
    // Key handling
    // ------------------------------------------------------------------

    fn handle_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        self.last_click = None;
        if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
            self.request_quit();
            return;
        }

        if self.modal.is_some() {
            self.handle_modal_key(key);
            return;
        }
        if self.editor.is_some() {
            self.handle_editor_key(key);
        } else {
            self.handle_connect_key(key);
        }
    }

    fn handle_connect_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Tab | KeyCode::Right | KeyCode::Left => {
                self.connect.focus = match self.connect.focus {
                    Panel::Usb => Panel::Bluetooth,
                    Panel::Bluetooth => Panel::Usb,
                };
            }
            KeyCode::Up => match self.connect.focus {
                Panel::Usb => move_list(&mut self.connect.usb_state, self.connect.usb.len(), -1),
                Panel::Bluetooth => move_list(
                    &mut self.connect.bluetooth_state,
                    self.connect.bluetooth.len(),
                    -1,
                ),
            },
            KeyCode::Down => match self.connect.focus {
                Panel::Usb => move_list(&mut self.connect.usb_state, self.connect.usb.len(), 1),
                Panel::Bluetooth => move_list(
                    &mut self.connect.bluetooth_state,
                    self.connect.bluetooth.len(),
                    1,
                ),
            },
            KeyCode::Enter => self.connect_selected(),
            KeyCode::Char('r') => {
                self.refresh_usb();
                self.spawn_bluetooth_scan();
            }
            KeyCode::Char('q') => self.request_quit(),
            _ => {}
        }
    }

    fn handle_editor_key(&mut self, key: KeyEvent) {
        if self.handle_editor_navigation(key) {
            return;
        }
        self.handle_editor_command(key);
    }

    /// Movement, layer cycling and numeric layer jumps. Returns true when the
    /// key was consumed.
    fn handle_editor_navigation(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Left | KeyCode::Char('h') => self.navigate(Direction::Left),
            KeyCode::Right | KeyCode::Char('l') => self.navigate(Direction::Right),
            KeyCode::Up | KeyCode::Char('k') => self.navigate(Direction::Up),
            KeyCode::Down | KeyCode::Char('j') => self.navigate(Direction::Down),
            KeyCode::Home | KeyCode::Char('g') => self.select_edge(false),
            KeyCode::End | KeyCode::Char('G') => self.select_edge(true),
            KeyCode::Tab | KeyCode::Char(']') => {
                self.cycle_layer(if key.modifiers.contains(KeyModifiers::SHIFT) {
                    -1
                } else {
                    1
                });
            }
            KeyCode::BackTab | KeyCode::Char('[') => self.cycle_layer(-1),
            KeyCode::Char(ch) if ch.is_ascii_digit() => {
                let index = ch.to_digit(10).unwrap_or(0) as usize;
                if let Some(editor) = &mut self.editor {
                    if index < editor.keymap.layers.len() {
                        editor.layer_index = index;
                        editor.clamp();
                    }
                }
            }
            _ => return false,
        }
        true
    }

    /// Commands that open a modal or act on the whole keymap.
    fn handle_editor_command(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => self.open_binding_editor(),
            KeyCode::Char('L') => self.open_layer_manager(),
            KeyCode::Char('b') => self.request_back_to_devices(),
            KeyCode::Char('p') => self.open_layout_picker(),
            KeyCode::Char('s') => {
                if self.require_unlocked() {
                    self.spawn_save(AfterSave::Stay);
                }
            }
            KeyCode::Char('d') => {
                if self.require_unlocked() {
                    self.modal = Some(Modal::Confirm(ConfirmModal::discard_changes()));
                }
            }
            KeyCode::Char('u') => self.open_unlock_modal(),
            KeyCode::Char('R') => {
                if self.require_unlocked() {
                    self.modal = Some(Modal::Confirm(ConfirmModal::reset_settings()));
                }
            }
            KeyCode::Char('?') => self.modal = Some(Modal::Help),
            KeyCode::Char('q') => self.request_quit(),
            KeyCode::Esc => self.status = None,
            _ => {}
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => self.mouse_down(mouse.column, mouse.row),
            MouseEventKind::ScrollUp => self.mouse_scroll(mouse.column, mouse.row, true),
            MouseEventKind::ScrollDown => self.mouse_scroll(mouse.column, mouse.row, false),
            _ => {}
        }
    }

    fn mouse_down(&mut self, column: u16, row: u16) {
        let Some(target) = self.mouse.hit(column, row) else {
            self.last_click = None;
            return;
        };

        let immediate = matches!(
            target,
            MouseTarget::LayerTab(_)
                | MouseTarget::ConfirmChoice(_)
                | MouseTarget::BindingApply
                | MouseTarget::EditorSave
                | MouseTarget::EditorDiscard
                | MouseTarget::EditorQuit
                | MouseTarget::HelpClose
                | MouseTarget::UnlockCheck
                | MouseTarget::ConnectPanel(_)
        );
        let double = self
            .last_click
            .map(|(previous, at)| previous == target && at.elapsed() <= Duration::from_millis(450))
            .unwrap_or(false);

        self.select_mouse_target(target);
        if immediate || double {
            self.activate_mouse_target(target);
            self.last_click = None;
        } else {
            self.last_click = Some((target, Instant::now()));
        }
    }

    fn select_mouse_target(&mut self, target: MouseTarget) {
        match target {
            MouseTarget::EditorKey(position) => {
                if let Some(editor) = &mut self.editor {
                    editor.selected = position;
                    editor.clamp();
                }
            }
            MouseTarget::UsbDevice(index) => {
                self.connect.focus = Panel::Usb;
                self.connect.usb_state.select(Some(index));
            }
            MouseTarget::BluetoothDevice(index) => {
                self.connect.focus = Panel::Bluetooth;
                self.connect.bluetooth_state.select(Some(index));
            }
            MouseTarget::ConnectPanel(panel) => self.connect.focus = panel,
            MouseTarget::BindingBehavior(index) => {
                if let Some(Modal::Binding(wizard)) = &mut self.modal {
                    wizard.behavior_state.select(Some(index));
                }
            }
            MouseTarget::BindingParam(index) => {
                if let Some(Modal::Binding(wizard)) = &mut self.modal {
                    wizard.param_state.select(Some(index));
                }
            }
            MouseTarget::LayerRow(index) => {
                if let Some(Modal::Layers(layers)) = &mut self.modal {
                    layers.state.select(Some(index));
                }
            }
            MouseTarget::LayoutRow(index) => {
                if let Some(Modal::Layouts(layouts)) = &mut self.modal {
                    layouts.state.select(Some(index));
                }
            }
            MouseTarget::LayerTab(_)
            | MouseTarget::ConfirmChoice(_)
            | MouseTarget::BindingApply
            | MouseTarget::EditorSave
            | MouseTarget::EditorDiscard
            | MouseTarget::EditorQuit
            | MouseTarget::HelpClose
            | MouseTarget::UnlockCheck => {}
        }
    }

    fn activate_mouse_target(&mut self, target: MouseTarget) {
        match target {
            MouseTarget::EditorKey(_) => self.open_binding_editor(),
            MouseTarget::UsbDevice(_) | MouseTarget::BluetoothDevice(_) => self.connect_selected(),
            MouseTarget::LayerTab(index) => {
                if let Some(editor) = &mut self.editor {
                    if index < editor.keymap.layers.len() {
                        editor.layer_index = index;
                        editor.clamp();
                    }
                }
            }
            MouseTarget::ConnectPanel(_) => {}
            MouseTarget::BindingBehavior(_)
            | MouseTarget::BindingParam(_)
            | MouseTarget::LayerRow(_)
            | MouseTarget::LayoutRow(_) => self.press_modal_key(KeyCode::Enter),
            MouseTarget::ConfirmChoice(index) => {
                let action = if let Some(Modal::Confirm(confirm)) = &self.modal {
                    confirm
                        .choices
                        .get(index)
                        .map(|choice| choice.action.clone())
                } else {
                    None
                };
                if let Some(action) = action {
                    self.apply_action(action);
                }
            }
            MouseTarget::BindingApply => self.press_modal_key(KeyCode::Enter),
            MouseTarget::EditorSave => {
                if self.require_unlocked() {
                    self.spawn_save(AfterSave::Stay);
                }
            }
            MouseTarget::EditorDiscard => {
                if self.require_unlocked() {
                    self.modal = Some(Modal::Confirm(ConfirmModal::discard_changes()));
                }
            }
            MouseTarget::EditorQuit => self.request_quit(),
            MouseTarget::HelpClose => self.modal = None,
            MouseTarget::UnlockCheck => self.press_modal_key(KeyCode::Enter),
        }
    }

    fn mouse_scroll(&mut self, column: u16, row: u16, up: bool) {
        if let Some(kind) = self.mouse.list_at(column, row) {
            match kind {
                ListKind::UsbDevices => {
                    self.connect.focus = Panel::Usb;
                    move_list(
                        &mut self.connect.usb_state,
                        self.connect.usb.len(),
                        if up { -1 } else { 1 },
                    );
                }
                ListKind::BluetoothDevices => {
                    self.connect.focus = Panel::Bluetooth;
                    move_list(
                        &mut self.connect.bluetooth_state,
                        self.connect.bluetooth.len(),
                        if up { -1 } else { 1 },
                    );
                }
                _ => self.press_modal_key(if up { KeyCode::Up } else { KeyCode::Down }),
            }
            self.last_click = None;
            return;
        }

        if self.modal.is_none() {
            self.navigate(if up { Direction::Up } else { Direction::Down });
            self.last_click = None;
        }
    }

    fn press_modal_key(&mut self, code: KeyCode) {
        if self.modal.is_some() {
            self.handle_modal_key(KeyEvent::new(code, KeyModifiers::NONE));
        }
    }

    fn handle_modal_key(&mut self, key: KeyEvent) {
        let action = {
            let (Some(modal), Some(editor)) = (self.modal.as_mut(), self.editor.as_ref()) else {
                self.modal = None;
                return;
            };
            modal.handle_key(key, &editor.keymap, &editor.catalog)
        };
        if let Some(action) = action {
            self.apply_action(action);
        }
    }

    fn apply_action(&mut self, action: UiAction) {
        match action {
            UiAction::Notice(message) => self.toast(message, StatusKind::Warning),
            UiAction::ApplyBinding {
                layer_id,
                position,
                binding,
            } => {
                if self.require_unlocked() {
                    self.modal = None;
                    self.spawn_set_binding(layer_id, position, binding);
                }
            }
            action @ (UiAction::AddLayer
            | UiAction::AskRemoveLayer(_)
            | UiAction::RemoveLayer(_)
            | UiAction::MoveLayer { .. }
            | UiAction::RenameLayer { .. }
            | UiAction::SelectLayer(_)) => self.apply_layer_action(action),
            UiAction::SetLayout(index) => self.apply_layout_action(index),
            action @ (UiAction::Save
            | UiAction::Discard
            | UiAction::ResetSettings
            | UiAction::Quit
            | UiAction::QuitSave
            | UiAction::QuitDiscard
            | UiAction::QuitAnyway
            | UiAction::DevicesSave
            | UiAction::DevicesDiscard
            | UiAction::DevicesAnyway
            | UiAction::Close
            | UiAction::CheckUnlock) => self.apply_session_action(action),
        }
    }

    fn apply_layer_action(&mut self, action: UiAction) {
        match action {
            UiAction::AddLayer => {
                if self.require_unlocked() {
                    self.modal = None;
                    self.spawn_add_layer();
                }
            }
            UiAction::AskRemoveLayer(index) => {
                if self.require_unlocked() {
                    if let Some(editor) = &self.editor {
                        if let Some(layer) = editor.keymap.layers.get(index) {
                            let name = layer.label(index);
                            self.modal =
                                Some(Modal::Confirm(ConfirmModal::remove_layer(index, &name)));
                        }
                    }
                }
            }
            UiAction::RemoveLayer(index) => {
                if self.require_unlocked() {
                    self.modal = None;
                    self.spawn_remove_layer(index);
                }
            }
            UiAction::MoveLayer { from, to } => {
                if self.require_unlocked() {
                    self.modal = None;
                    self.spawn_move_layer(from, to);
                }
            }
            UiAction::RenameLayer { layer_id, name } => {
                if self.require_unlocked() {
                    self.modal = None;
                    self.spawn_rename_layer(layer_id, name);
                }
            }
            UiAction::SelectLayer(index) => {
                if let Some(editor) = &mut self.editor {
                    if index < editor.keymap.layers.len() {
                        editor.layer_index = index;
                        editor.clamp();
                    }
                }
                self.modal = None;
            }
            _ => unreachable!("only layer actions reach this handler"),
        }
    }

    fn apply_layout_action(&mut self, index: usize) {
        if self.require_unlocked() {
            let valid = self
                .editor
                .as_ref()
                .map(|editor| index < editor.layouts.layouts.len())
                .unwrap_or(false);
            if valid {
                self.modal = None;
                self.spawn_set_layout(index);
            } else {
                self.modal = None;
            }
        }
    }

    fn apply_session_action(&mut self, action: UiAction) {
        match action {
            action @ (UiAction::Save | UiAction::Discard | UiAction::ResetSettings) => {
                self.apply_save_action(action)
            }
            action @ (UiAction::Quit
            | UiAction::QuitSave
            | UiAction::QuitDiscard
            | UiAction::QuitAnyway
            | UiAction::DevicesSave
            | UiAction::DevicesDiscard
            | UiAction::DevicesAnyway
            | UiAction::Close
            | UiAction::CheckUnlock) => self.apply_quit_action(action),
            _ => unreachable!("only session actions reach this handler"),
        }
    }

    fn apply_save_action(&mut self, action: UiAction) {
        match action {
            UiAction::Save => {
                if self.require_unlocked() {
                    self.modal = None;
                    self.spawn_save(AfterSave::Stay);
                }
            }
            UiAction::Discard => {
                if self.require_unlocked() {
                    self.modal = None;
                    self.spawn_discard(AfterSave::Stay);
                }
            }
            UiAction::ResetSettings => {
                if self.require_unlocked() {
                    self.modal = None;
                    self.spawn_reset_settings();
                }
            }
            _ => unreachable!("only save/discard/reset actions reach this handler"),
        }
    }

    fn apply_quit_action(&mut self, action: UiAction) {
        match action {
            UiAction::Quit => self.request_quit(),
            UiAction::QuitSave => {
                if self.require_unlocked() {
                    self.modal = None;
                    self.spawn_save(AfterSave::Quit);
                }
            }
            UiAction::QuitDiscard => {
                if self.require_unlocked() {
                    self.modal = None;
                    self.spawn_discard(AfterSave::Quit);
                }
            }
            UiAction::QuitAnyway => self.should_quit = true,
            UiAction::DevicesSave => {
                if self.require_unlocked() {
                    self.modal = None;
                    self.spawn_save(AfterSave::Devices);
                }
            }
            UiAction::DevicesDiscard => {
                if self.require_unlocked() {
                    self.modal = None;
                    self.spawn_discard(AfterSave::Devices);
                }
            }
            UiAction::DevicesAnyway => {
                self.modal = None;
                self.leave_editor();
                self.toast("Choose a keyboard", StatusKind::Info);
            }
            UiAction::Close => self.modal = None,
            UiAction::CheckUnlock => {
                if let Some(Modal::Unlock(unlock)) = &mut self.modal {
                    unlock.waiting = true;
                }
                self.spawn_lock_poll();
            }
            _ => unreachable!("only quit/close/unlock actions reach this handler"),
        }
    }

    // ------------------------------------------------------------------
    // Editor actions
    // ------------------------------------------------------------------

    fn navigate(&mut self, direction: Direction) {
        let viewport = self.keyboard_viewport;
        let Some(editor) = &mut self.editor else {
            return;
        };
        let physical = editor.effective_layout();
        let next = if viewport.width > 0 && viewport.height > 0 {
            // Navigate on the rendered grid so column stagger does not turn a
            // horizontal step into an "up" move.
            layout::compute(&physical, viewport).move_selection(
                &physical,
                editor.selected,
                direction,
            )
        } else {
            layout::move_selection(&physical, editor.selected, direction)
        };
        if let Some(next) = next {
            editor.selected = next;
        }
    }

    fn select_edge(&mut self, end: bool) {
        if let Some(editor) = &mut self.editor {
            let count = editor.position_count();
            editor.selected = if end { count.saturating_sub(1) } else { 0 };
        }
    }

    fn cycle_layer(&mut self, delta: isize) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let count = editor.keymap.layers.len();
        if count == 0 {
            return;
        }
        editor.layer_index =
            (editor.layer_index as isize + delta).rem_euclid(count as isize) as usize;
        editor.clamp();
    }

    fn open_binding_editor(&mut self) {
        if !self.require_unlocked() {
            return;
        }
        let Some(editor) = &self.editor else {
            return;
        };
        if editor.keymap.layers.is_empty() {
            self.toast("No keymap data loaded yet", StatusKind::Warning);
            return;
        }
        let Some(layer) = editor.keymap.layers.get(editor.layer_index) else {
            return;
        };
        if editor.selected >= layer.bindings.len() {
            self.toast("This position has no binding", StatusKind::Warning);
            return;
        }
        self.modal = Some(Modal::Binding(BindingWizard::new(
            &editor.keymap,
            &editor.catalog,
            editor.layer_index,
            editor.selected,
        )));
    }

    fn open_layer_manager(&mut self) {
        let Some(editor) = &self.editor else {
            return;
        };
        if !editor.capabilities.manage_layers {
            self.toast(
                "This Vial keyboard has a fixed layer count; only bindings can be edited",
                StatusKind::Warning,
            );
            return;
        }
        if editor.keymap.layers.is_empty() {
            self.toast("No layers loaded yet", StatusKind::Warning);
            return;
        }
        self.modal = Some(Modal::Layers(LayerModal::new(
            editor.layer_index,
            editor.keymap.layers.len(),
        )));
    }

    fn open_layout_picker(&mut self) {
        let Some(editor) = &self.editor else {
            return;
        };
        if editor.layouts.layouts.len() <= 1 {
            self.toast(
                "This keyboard reports a single physical layout",
                StatusKind::Info,
            );
            return;
        }
        self.modal = Some(Modal::Layouts(LayoutModal::new(
            editor.layouts.active_index,
            editor.layouts.layouts.len(),
        )));
    }

    fn open_unlock_modal(&mut self) {
        self.modal = Some(Modal::Unlock(UnlockModal::default()));
        self.spawn_lock_poll();
    }

    fn require_unlocked(&mut self) -> bool {
        if let Some(editor) = &self.editor {
            if editor.locked {
                self.toast(
                    "Keyboard is locked — press your Studio Unlock key combination",
                    StatusKind::Warning,
                );
                self.modal = Some(Modal::Unlock(UnlockModal::default()));
                return false;
            }
        }
        true
    }

    fn request_back_to_devices(&mut self) {
        if self.editor.as_ref().is_some_and(|editor| editor.dirty) {
            self.modal = Some(Modal::Confirm(ConfirmModal::devices()));
            return;
        }
        self.leave_editor();
        self.toast("Choose a keyboard", StatusKind::Info);
    }

    /// Drop the current device connection and return to a fresh device list.
    /// Dropping the client closes the serial port, BLE link or raw-HID handle;
    /// the next screen scans again instead of reusing stale state.
    fn leave_editor(&mut self) {
        self.auto_connect = false;
        self.editor = None;
        self.modal = None;
        self.busy = None;
        self.connect_inflight = false;
        self.scanning_inflight = false;
        self.snapshot_retry_at = None;
        self.snapshot_attempts = 0;
        self.pending_layer_select = None;
        self.last_click = None;
        self.simulator = None;
        self.connect = ConnectState::default();
        self.refresh_usb();
        self.spawn_bluetooth_scan();
    }

    fn request_quit(&mut self) {
        if let Some(editor) = &self.editor {
            if editor.dirty {
                self.modal = Some(Modal::Confirm(ConfirmModal::quit()));
                return;
            }
        }
        self.should_quit = true;
    }

    // ------------------------------------------------------------------
    // Device connection
    // ------------------------------------------------------------------

    fn refresh_usb(&mut self) {
        self.connect.usb = if self.start.scan_vial {
            list_usb_entries()
        } else {
            serial::list_usb_devices()
                .into_iter()
                .map(UsbEntry::Studio)
                .collect()
        };
        self.clamp_connect_selection();
    }

    fn clamp_connect_selection(&mut self) {
        clamp_list(&mut self.connect.usb_state, self.connect.usb.len());
        clamp_list(
            &mut self.connect.bluetooth_state,
            self.connect.bluetooth.len(),
        );
        if self.connect.usb.is_empty() && !self.connect.bluetooth.is_empty() {
            self.connect.focus = Panel::Bluetooth;
        }
    }

    /// The single keyboard the app may connect to without asking, if any.
    /// More than one USB/HID candidate is genuinely ambiguous, so the connect
    /// screen stays up and the user picks.
    pub fn auto_connect_choice(&self) -> Option<DeviceChoice> {
        if self.connect.usb.len() == 1 {
            return Some(self.connect.usb[0].choice());
        }
        if !self.connect.usb.is_empty() {
            return None;
        }
        if self.connect.bluetooth.len() == 1 {
            return Some(DeviceChoice::Bluetooth {
                device: self.connect.bluetooth[0].clone(),
            });
        }
        let mut connected = self
            .connect
            .bluetooth
            .iter()
            .filter(|device| device.connected);
        match (connected.next(), connected.next()) {
            (Some(device), None) => Some(DeviceChoice::Bluetooth {
                device: device.clone(),
            }),
            _ => None,
        }
    }

    fn try_auto_connect(&mut self) -> bool {
        if !self.auto_connect || self.connect_inflight || self.editor.is_some() {
            return false;
        }
        let Some(choice) = self.auto_connect_choice() else {
            return false;
        };
        // One attempt only: a failed automatic connection drops to the connect
        // list instead of retrying forever in the background.
        self.auto_connect = false;
        self.spawn_connect(choice);
        true
    }

    fn connect_selected(&mut self) {
        let choice = match self.connect.focus {
            Panel::Usb => {
                let index = self.connect.usb_state.selected().unwrap_or(0);
                match self.connect.usb.get(index) {
                    Some(entry) => Some(entry.choice()),
                    None => {
                        self.toast("Select a USB keyboard first", StatusKind::Warning);
                        None
                    }
                }
            }
            Panel::Bluetooth => {
                let index = self.connect.bluetooth_state.selected().unwrap_or(0);
                match self.connect.bluetooth.get(index) {
                    Some(device) => Some(DeviceChoice::Bluetooth {
                        device: device.clone(),
                    }),
                    None => {
                        self.toast("Select a Bluetooth keyboard first", StatusKind::Warning);
                        None
                    }
                }
            }
        };
        if let Some(choice) = choice {
            self.spawn_connect(choice);
        }
    }

    fn spawn_connect(&mut self, choice: DeviceChoice) {
        if self.connect_inflight {
            return;
        }
        self.auto_connect = false;
        self.connect_inflight = true;
        self.busy = Some(format!(
            "Connecting to {} · {}…",
            choice.label(),
            choice.standard()
        ));
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = connect_choice(choice, tx.clone()).await;
            let _ = tx.send(Msg::ConnectResult(result));
        });
    }

    fn spawn_bluetooth_by_name(&mut self, name: String) {
        if self.connect_inflight {
            return;
        }
        self.connect_inflight = true;
        self.busy = Some(format!("Looking for “{name}” over Bluetooth…"));
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = connect_by_bluetooth_name(&name, tx.clone()).await;
            let _ = tx.send(Msg::ConnectResult(result));
        });
    }

    fn start_simulator(&mut self, board: SimulatedBoard) {
        let (simulator, connection) = Simulator::start(board, false);
        self.simulator = Some(simulator);
        self.connect_inflight = true;
        self.busy = Some(format!("Starting the {} simulator…", board.label()));
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = finish_connection(connection, tx.clone()).await;
            let _ = tx.send(Msg::ConnectResult(result));
        });
    }

    fn spawn_bluetooth_scan(&mut self) {
        if self.scanning_inflight {
            return;
        }
        self.scanning_inflight = true;
        self.connect.scanning = true;
        self.connect.scan_error = None;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = scan_bluetooth().await;
            let _ = tx.send(Msg::BluetoothDevices(result));
        });
    }

    fn handle_connect_result(&mut self, result: Result<Connected, String>) {
        self.connect_inflight = false;
        self.busy = None;
        self.snapshot_retry_at = None;
        self.snapshot_attempts = 0;
        self.pending_layer_select = None;
        // Leaving the connect screen invalidates the scan bookkeeping: an
        // in-flight scan must not block the scan that a later disconnect needs.
        self.scanning_inflight = false;
        self.connect.scanning = false;
        match result {
            Err(error) => {
                self.toast(error, StatusKind::Error);
                // Drop the old scan handles so a later attempt uses a fresh
                // CoreBluetooth peripheral/central rather than stale state.
                self.connect = ConnectState::default();
                self.refresh_usb();
                self.spawn_bluetooth_scan();
            }
            Ok(connected) => {
                let warnings = connected
                    .snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.warnings.clone())
                    .unwrap_or_default();
                let name = connected.info.name.clone();
                let has_snapshot = connected.snapshot.is_some();
                let mut editor = EditorState::new(connected);
                // Lock notifications that arrived during the handshake were
                // forwarded before the editor existed and therefore dropped;
                // the client cached their state, so reconcile with it now.
                let cached_lock = editor.client.try_cached_lock_state().flatten();
                if let Some(cached) = cached_lock {
                    if cached.is_locked() != editor.locked {
                        editor.locked = cached.is_locked();
                    }
                }
                let locked = editor.locked;
                let layout = editor.active_layout_name().to_string();
                self.editor = Some(editor);
                self.connect = ConnectState::default();
                self.modal = if locked {
                    Some(Modal::Unlock(UnlockModal::default()))
                } else {
                    None
                };
                if !locked && !has_snapshot {
                    self.spawn_snapshot_load();
                }
                self.last_lock_poll = Instant::now();
                if !warnings.is_empty() {
                    self.toast(
                        format!("Connected to {name}; {} warning(s)", warnings.len()),
                        StatusKind::Warning,
                    );
                } else if locked {
                    self.toast(
                        "Keyboard is locked — press your Studio Unlock key combination",
                        StatusKind::Warning,
                    );
                } else {
                    self.toast(
                        format!("Connected to {name} · layout {layout}"),
                        StatusKind::Success,
                    );
                }
            }
        }
    }

    fn handle_device_event(&mut self, event: StudioEvent) {
        match event {
            StudioEvent::LockStateChanged(lock) => {
                let Some(editor) = &mut self.editor else {
                    return;
                };
                editor.locked = lock.is_locked();
                if lock.is_locked() {
                    if self.modal.is_none() {
                        self.modal = Some(Modal::Unlock(UnlockModal::default()));
                    }
                    self.toast("Keyboard locked", StatusKind::Warning);
                } else {
                    if matches!(self.modal, Some(Modal::Unlock(_))) {
                        self.modal = None;
                    }
                    if !editor.keymap_loaded {
                        self.spawn_snapshot_load();
                    }
                    self.toast("Keyboard unlocked", StatusKind::Success);
                }
            }
            StudioEvent::UnsavedChangesChanged(dirty) => {
                if let Some(editor) = &mut self.editor {
                    editor.dirty = dirty;
                }
            }
            StudioEvent::Disconnected(reason) => self.handle_disconnect(reason),
        }
    }

    fn handle_lock_polled(&mut self, result: Result<LockState, StudioError>) {
        self.lock_poll_inflight = false;
        if let Some(Modal::Unlock(unlock)) = &mut self.modal {
            unlock.waiting = false;
        }
        match result {
            Ok(LockState::Unlocked) => {
                let Some(editor) = &mut self.editor else {
                    return;
                };
                editor.locked = false;
                let needs_load = !editor.keymap_loaded;
                if matches!(self.modal, Some(Modal::Unlock(_))) {
                    self.modal = None;
                }
                self.toast("Keyboard unlocked", StatusKind::Success);
                if needs_load {
                    self.spawn_snapshot_load();
                }
            }
            Ok(LockState::Locked) => {
                if let Some(editor) = &mut self.editor {
                    editor.locked = true;
                }
            }
            Err(error) => self.handle_studio_error(error),
        }
    }

    fn handle_disconnect(&mut self, reason: String) {
        self.snapshot_retry_at = None;
        self.snapshot_attempts = 0;
        if self.editor.is_none() {
            return;
        }
        self.editor = None;
        self.modal = None;
        self.busy = None;
        self.simulator = None;
        self.connect_inflight = false;
        self.lock_poll_inflight = false;
        self.pending_layer_select = None;
        self.scanning_inflight = false;
        self.toast(format!("Disconnected: {reason}"), StatusKind::Error);
        self.connect = ConnectState::default();
        self.refresh_usb();
        self.spawn_bluetooth_scan();
    }

    // ------------------------------------------------------------------
    // Background work
    // ------------------------------------------------------------------

    fn on_tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        if let Some(status) = &self.status {
            if status.kind != StatusKind::Error && status.at.elapsed() > Duration::from_secs(6) {
                self.status = None;
            }
        }
        let should_poll = self
            .editor
            .as_ref()
            .map(|editor| {
                editor.locked
                    && !self.lock_poll_inflight
                    && self.last_lock_poll.elapsed() > Duration::from_millis(1500)
            })
            .unwrap_or(false);
        if should_poll {
            self.spawn_lock_poll();
        }
        self.retry_snapshot_if_due();
    }

    /// Retry a failed initial keymap load. Without this, one timeout or
    /// transient error leaves the editor showing its loading spinner forever:
    /// the only other trigger is an unlock notification.
    fn retry_snapshot_if_due(&mut self) {
        let Some(retry_at) = self.snapshot_retry_at else {
            return;
        };
        let loaded = self
            .editor
            .as_ref()
            .map(|editor| editor.keymap_loaded)
            .unwrap_or(false);
        if self.editor.is_none() || loaded {
            self.snapshot_retry_at = None;
            self.snapshot_attempts = 0;
            return;
        }
        if Instant::now() < retry_at || self.busy.is_some() {
            return;
        }
        self.snapshot_retry_at = None;
        self.snapshot_attempts += 1;
        if self.snapshot_attempts > MAX_SNAPSHOT_RETRIES {
            // A persistent failure should not turn into a request loop; leave a
            // message that says how to retry manually.
            self.toast(
                "Could not read the keymap — press u to retry",
                StatusKind::Error,
            );
            return;
        }
        self.start_snapshot_load(false);
    }

    fn schedule_snapshot_retry(&mut self) {
        self.snapshot_retry_at = Some(Instant::now() + SNAPSHOT_RETRY_DELAY);
    }

    fn spawn_lock_poll(&mut self) {
        if self.lock_poll_inflight {
            return;
        }
        let Some(client) = self.client() else {
            return;
        };
        self.lock_poll_inflight = true;
        self.last_lock_poll = Instant::now();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = client.get_lock_state().await;
            let _ = tx.send(Msg::LockPolled(result));
        });
    }

    fn spawn_snapshot_load(&mut self) {
        self.start_snapshot_load(true);
    }

    fn start_snapshot_load(&mut self, reset_attempts: bool) {
        let Some(client) = self.client() else {
            return;
        };
        if self.busy.is_some() {
            // Another request owns the pipe; try again once it has finished.
            self.schedule_snapshot_retry();
            return;
        }
        self.snapshot_retry_at = None;
        if reset_attempts {
            self.snapshot_attempts = 0;
        }
        self.snapshot_generation = self.mutation_generation;
        self.busy = Some("Reading keymap from keyboard…".into());
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = load_snapshot(&client).await;
            let _ = tx.send(Msg::Snapshot(result));
        });
    }

    fn handle_snapshot(&mut self, result: Result<Snapshot, StudioError>) {
        match result {
            Ok(snapshot) => {
                let warnings = snapshot.warnings.clone();
                // The load is a sequence of RPCs; edits made while it ran are
                // newer than the payload. Keep the newer local keymap and dirty
                // state in that case (the catalog and layouts are never edited
                // locally, so they are always safe to apply).
                let stale = self.snapshot_generation != self.mutation_generation;
                if let Some(editor) = &mut self.editor {
                    if !stale {
                        editor.keymap = snapshot.keymap;
                        editor.dirty = snapshot.dirty;
                    }
                    editor.layouts = snapshot.layouts;
                    editor.catalog = snapshot.catalog;
                    editor.keymap_loaded = true;
                    editor.clamp();
                }
                self.snapshot_attempts = 0;
                // A successful load does not prove the keyboard is still
                // unlocked: a lock notification may have arrived while it ran
                // and must not be overwritten here.
                let locked = self
                    .editor
                    .as_ref()
                    .map(|editor| editor.locked)
                    .unwrap_or(false);
                if !locked && matches!(self.modal, Some(Modal::Unlock(_))) {
                    self.modal = None;
                }
                self.pending_layer_select = None;
                self.busy = None;
                let layout = self
                    .editor
                    .as_ref()
                    .map(|editor| editor.active_layout_name().to_string())
                    .unwrap_or_default();
                if warnings.is_empty() {
                    self.toast(
                        format!("Keymap loaded · layout {layout}"),
                        StatusKind::Success,
                    );
                } else {
                    self.toast(
                        format!("Keymap loaded; {} warning(s)", warnings.len()),
                        StatusKind::Warning,
                    );
                }
            }
            Err(StudioError::UnlockRequired) => {
                if let Some(editor) = &mut self.editor {
                    editor.locked = true;
                }
                self.busy = None;
                self.snapshot_attempts = 0;
                self.modal = Some(Modal::Unlock(UnlockModal::default()));
            }
            Err(error) => {
                self.handle_studio_error(error);
                // The device is reachable but the load failed (usually a
                // timeout); schedule a retry so the UI cannot stay stuck on
                // "Reading keymap…" forever.
                if self.editor.is_some() {
                    self.schedule_snapshot_retry();
                }
            }
        }
    }

    fn spawn_set_binding(&mut self, layer_id: u32, position: usize, binding: Binding) {
        let Some(client) = self.client() else {
            return;
        };
        self.begin_mutation();
        self.busy = Some("Updating binding…".into());
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = client.set_layer_binding(layer_id, position, binding).await;
            let _ = tx.send(Msg::BindingSet {
                result,
                layer_id,
                position,
                binding,
            });
        });
    }

    fn spawn_save(&mut self, then: AfterSave) {
        let Some(client) = self.client() else {
            return;
        };
        self.busy = Some(match then {
            AfterSave::Quit => "Saving before quitting…".into(),
            AfterSave::Devices => "Saving before returning…".into(),
            AfterSave::Stay => "Saving…".into(),
        });
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = client.save_changes().await;
            let _ = tx.send(Msg::Saved { result, then });
        });
    }

    fn spawn_discard(&mut self, then: AfterSave) {
        let Some(client) = self.client() else {
            return;
        };
        self.busy = Some("Discarding changes…".into());
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = client.discard_changes().await;
            let _ = tx.send(Msg::Discarded { result, then });
        });
    }

    fn spawn_reset_settings(&mut self) {
        let Some(client) = self.client() else {
            return;
        };
        self.busy = Some("Restoring stock settings…".into());
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = client.reset_settings().await;
            let _ = tx.send(Msg::ResetSettings(result));
        });
    }

    fn spawn_add_layer(&mut self) {
        let Some(client) = self.client() else {
            return;
        };
        self.begin_mutation();
        self.busy = Some("Adding layer…".into());
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = client.add_layer().await;
            let _ = tx.send(Msg::LayerAdded(result));
        });
    }

    fn spawn_remove_layer(&mut self, index: usize) {
        let Some(client) = self.client() else {
            return;
        };
        self.begin_mutation();
        self.busy = Some("Removing layer…".into());
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = client.remove_layer(index).await;
            let _ = tx.send(Msg::LayerRemoved { result, index });
        });
    }

    fn spawn_move_layer(&mut self, from: usize, to: usize) {
        let Some(client) = self.client() else {
            return;
        };
        self.begin_mutation();
        self.busy = Some("Moving layer…".into());
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = client.move_layer(from, to).await;
            let _ = tx.send(Msg::LayerMoved { result, to });
        });
    }

    fn spawn_rename_layer(&mut self, layer_id: u32, name: String) {
        let Some(client) = self.client() else {
            return;
        };
        self.begin_mutation();
        self.busy = Some("Renaming layer…".into());
        let tx = self.tx.clone();
        let sent_name = name.clone();
        tokio::spawn(async move {
            let result = client.set_layer_props(layer_id, &sent_name).await;
            let _ = tx.send(Msg::LayerRenamed {
                result,
                layer_id,
                name,
            });
        });
    }

    fn spawn_set_layout(&mut self, index: usize) {
        let Some(client) = self.client() else {
            return;
        };
        if self
            .editor
            .as_ref()
            .is_some_and(|editor| editor.layouts.active_index != index)
        {
            self.begin_mutation();
        }
        self.busy = Some("Changing physical layout…".into());
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = client.set_active_physical_layout(index).await;
            let _ = tx.send(Msg::LayoutChanged { result, index });
        });
    }

    fn spawn_refresh_keymap(&mut self) {
        let Some(client) = self.client() else {
            return;
        };
        self.busy = Some("Refreshing keymap…".into());
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = client.get_keymap().await;
            let _ = tx.send(Msg::KeymapRefreshed(result));
        });
    }

    /// Refresh the keymap and the physical-layout selection together. Called
    /// after a discard, because the firmware reverts its selected physical
    /// layout as part of discarding and a keymap-only refresh would leave the
    /// editor drawing the layout that was just reverted.
    fn spawn_refresh_edit(&mut self) {
        let Some(client) = self.client() else {
            return;
        };
        self.busy = Some("Refreshing keymap…".into());
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = async {
                let keymap = client.get_keymap().await?;
                let layouts = match client.get_physical_layouts().await {
                    Ok(layouts) => layouts,
                    // Firmware without the layout RPC keeps the generic grid.
                    Err(StudioError::RpcNotFound) => PhysicalLayouts::default(),
                    Err(error) => return Err(error),
                };
                Ok((keymap, layouts))
            }
            .await;
            let _ = tx.send(Msg::EditRefreshed(result));
        });
    }

    fn handle_studio_error(&mut self, error: StudioError) {
        self.busy = None;
        if error.is_disconnect() {
            let reason = match error {
                StudioError::Disconnected(reason) => reason,
                other => other.to_string(),
            };
            self.handle_disconnect(reason);
            return;
        }
        if matches!(error, StudioError::UnlockRequired) {
            if let Some(editor) = &mut self.editor {
                editor.locked = true;
            }
            self.modal = Some(Modal::Unlock(UnlockModal::default()));
            self.toast(
                "Keyboard is locked — press your Studio Unlock key combination",
                StatusKind::Warning,
            );
        } else {
            self.toast(error.to_string(), StatusKind::Error);
        }
    }

    fn client(&self) -> Option<DeviceClient> {
        self.editor.as_ref().map(|editor| editor.client.clone())
    }

    pub fn toast(&mut self, text: impl Into<String>, kind: StatusKind) {
        self.status = Some(Status {
            text: text.into(),
            kind,
            at: Instant::now(),
        });
    }
}

fn clamp_list(state: &mut ListState, len: usize) {
    if len == 0 {
        state.select(None);
        return;
    }
    let index = state.selected().unwrap_or(0).min(len - 1);
    state.select(Some(index));
}

fn move_list(state: &mut ListState, len: usize, delta: isize) {
    if len == 0 {
        state.select(None);
        return;
    }
    let current = state.selected().unwrap_or(0).min(len - 1) as isize;
    let next = (current + delta).clamp(0, len as isize - 1) as usize;
    state.select(Some(next));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::serial::UsbDevice;

    #[test]
    fn auto_connect_is_off_by_default() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(tx, StartOptions::default());
        app.connect.usb = vec![UsbEntry::Studio(UsbDevice {
            path: "/dev/zmk-tui-test".into(),
            product: Some("Test".into()),
            manufacturer: None,
            serial_number: None,
        })];
        assert!(!app.try_auto_connect());
        assert!(app.editor.is_none());
    }

    #[tokio::test]
    async fn auto_connect_dispatches_the_only_candidate() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            tx,
            StartOptions {
                auto_connect: true,
                scan_vial: false,
                ..Default::default()
            },
        );
        app.connect.usb = vec![UsbEntry::Studio(UsbDevice {
            path: "/dev/zmk-tui-test".into(),
            product: Some("Test".into()),
            manufacturer: None,
            serial_number: None,
        })];
        assert!(app.try_auto_connect());
        assert!(app.connect_inflight);
        assert!(!app.auto_connect, "automatic connection is one-shot");
    }
}
