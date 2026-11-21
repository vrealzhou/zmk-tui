//! ZMK Studio RPC client.
//!
//! One reader task owns the transport's incoming frames, matches responses to
//! pending requests by ID, and forwards notifications. Requests are serialised
//! so a device never sees overlapping writes.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use prost::Message;
use thiserror::Error;
use tokio::sync::{Mutex, RwLock, broadcast, oneshot};
use tokio::time::timeout;

use crate::model::{
    Behavior, Binding, Catalog, DeviceInfo, KeyGeometry, Keymap, Layer, LockState, ParamDesc,
    ParamKind, ParamSet, PhysicalLayout, PhysicalLayouts,
};
use crate::protocol::messages::zmk;
use crate::transport::Connection;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const EVENT_CHANNEL_CAPACITY: usize = 64;

#[derive(Debug, Error)]
pub enum StudioError {
    #[error("the keyboard is locked — press your Studio Unlock key combination")]
    UnlockRequired,
    #[error("the keyboard does not support this request")]
    RpcNotFound,
    #[error("the keyboard reported an error: {0}")]
    Device(String),
    #[error(
        "the keyboard did not respond in time — check the connection and close any other Studio client"
    )]
    Timeout,
    #[error("connection lost: {0}")]
    Disconnected(String),
    #[error("transport error: {0}")]
    Transport(String),
    #[error("unexpected response from the keyboard: {0}")]
    Unexpected(String),
    #[error("protocol error: {0}")]
    Protocol(String),
}

impl StudioError {
    /// True when the failure should send the user back to the connect screen.
    pub fn is_disconnect(&self) -> bool {
        matches!(self, Self::Disconnected(_) | Self::Transport(_))
    }
}

#[derive(Debug, Clone)]
pub enum StudioEvent {
    LockStateChanged(LockState),
    UnsavedChangesChanged(bool),
    Disconnected(String),
}

#[derive(Debug, Clone, Copy, Default)]
struct CachedState {
    connected: bool,
    lock: Option<LockState>,
    unsaved: Option<bool>,
}

#[derive(Clone)]
pub struct StudioClient {
    inner: Arc<Inner>,
}

struct Inner {
    sink: crate::transport::Sink,
    pending: Mutex<HashMap<u32, oneshot::Sender<zmk::studio::RequestResponse>>>,
    next_request_id: AtomicU32,
    request_lock: Mutex<()>,
    state: RwLock<CachedState>,
    events: broadcast::Sender<StudioEvent>,
}

impl StudioClient {
    /// Start the reader task for an established connection.
    pub fn start(connection: Connection) -> Self {
        let Connection { sink, frames, .. } = connection;
        let (events, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let inner = Arc::new(Inner {
            sink,
            pending: Mutex::new(HashMap::new()),
            next_request_id: AtomicU32::new(0),
            request_lock: Mutex::new(()),
            state: RwLock::new(CachedState {
                connected: true,
                ..Default::default()
            }),
            events,
        });

        let reader_inner = Arc::downgrade(&inner);
        tokio::spawn(read_loop(frames, reader_inner));
        Self { inner }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<StudioEvent> {
        self.inner.events.subscribe()
    }

    pub async fn cached_lock_state(&self) -> Option<LockState> {
        self.inner.state.read().await.lock
    }

    pub async fn cached_unsaved_changes(&self) -> Option<bool> {
        self.inner.state.read().await.unsaved
    }

    pub async fn is_connected(&self) -> bool {
        self.inner.state.read().await.connected
    }

    async fn request(
        &self,
        subsystem: zmk::studio::request::Subsystem,
    ) -> Result<zmk::studio::RequestResponse, StudioError> {
        let _serialised = self.inner.request_lock.lock().await;

        let request_id = self.inner.next_request_id.fetch_add(1, Ordering::Relaxed);
        let request = zmk::studio::Request {
            request_id,
            subsystem: Some(subsystem),
        };

        let (response_tx, response_rx) = oneshot::channel();
        self.inner
            .pending
            .lock()
            .await
            .insert(request_id, response_tx);

        if let Err(error) = self.inner.sink.send(request.encode_to_vec()).await {
            self.inner.pending.lock().await.remove(&request_id);
            return Err(StudioError::Transport(error.to_string()));
        }

        let response = match timeout(REQUEST_TIMEOUT, response_rx).await {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => {
                self.inner.pending.lock().await.remove(&request_id);
                return Err(StudioError::Disconnected(
                    "the connection closed while waiting for a response".into(),
                ));
            }
            Err(_) => {
                self.inner.pending.lock().await.remove(&request_id);
                return Err(StudioError::Timeout);
            }
        };

        if let Some(zmk::studio::request_response::Subsystem::Meta(meta)) = &response.subsystem {
            return Err(meta_error(meta));
        }
        Ok(response)
    }

    /// Retry a read once after a timeout. A late response from the first
    /// attempt is ignored because its pending entry was removed; retries are
    /// safe for these idempotent queries.
    async fn read_request(
        &self,
        subsystem: zmk::studio::request::Subsystem,
    ) -> Result<zmk::studio::RequestResponse, StudioError> {
        match self.request(subsystem.clone()).await {
            Err(StudioError::Timeout) => {
                tokio::time::sleep(Duration::from_millis(200)).await;
                self.request(subsystem).await
            }
            result => result,
        }
    }

    pub async fn get_device_info(&self) -> Result<DeviceInfo, StudioError> {
        let response = self
            .read_request(zmk::studio::request::Subsystem::Core(core_request(
                zmk::core::request::RequestType::GetDeviceInfo(true),
            )))
            .await?;
        match core_response(response)? {
            zmk::core::response::ResponseType::GetDeviceInfo(info) => Ok(DeviceInfo {
                name: info.name,
                serial_number: info.serial_number,
            }),
            other => Err(unexpected_core(&other)),
        }
    }

    pub async fn get_lock_state(&self) -> Result<LockState, StudioError> {
        let response = self
            .read_request(zmk::studio::request::Subsystem::Core(core_request(
                zmk::core::request::RequestType::GetLockState(true),
            )))
            .await?;
        let lock = match core_response(response)? {
            zmk::core::response::ResponseType::GetLockState(value) => proto_lock_state(value)
                .ok_or_else(|| {
                    StudioError::Protocol(format!("unknown lock state value {value}"))
                })?,
            other => return Err(unexpected_core(&other)),
        };
        self.set_lock_state(lock).await;
        Ok(lock)
    }

    pub async fn get_keymap(&self) -> Result<Keymap, StudioError> {
        let response = self
            .read_request(zmk::studio::request::Subsystem::Keymap(
                zmk::keymap::Request {
                    request_type: Some(zmk::keymap::request::RequestType::GetKeymap(true)),
                },
            ))
            .await?;
        match keymap_response(response)? {
            zmk::keymap::response::ResponseType::GetKeymap(keymap) => Ok(convert_keymap(keymap)),
            other => Err(unexpected_keymap(&other)),
        }
    }

    pub async fn get_physical_layouts(&self) -> Result<PhysicalLayouts, StudioError> {
        let response = self
            .read_request(zmk::studio::request::Subsystem::Keymap(
                zmk::keymap::Request {
                    request_type: Some(zmk::keymap::request::RequestType::GetPhysicalLayouts(true)),
                },
            ))
            .await?;
        match keymap_response(response)? {
            zmk::keymap::response::ResponseType::GetPhysicalLayouts(layouts) => {
                Ok(convert_layouts(layouts))
            }
            other => Err(unexpected_keymap(&other)),
        }
    }

    pub async fn check_unsaved_changes(&self) -> Result<bool, StudioError> {
        let response = self
            .read_request(zmk::studio::request::Subsystem::Keymap(
                zmk::keymap::Request {
                    request_type: Some(zmk::keymap::request::RequestType::CheckUnsavedChanges(
                        true,
                    )),
                },
            ))
            .await?;
        match keymap_response(response)? {
            zmk::keymap::response::ResponseType::CheckUnsavedChanges(unsaved) => {
                self.set_unsaved(unsaved).await;
                Ok(unsaved)
            }
            other => Err(unexpected_keymap(&other)),
        }
    }

    pub async fn list_behaviors(&self) -> Result<Vec<u32>, StudioError> {
        let response = self
            .read_request(zmk::studio::request::Subsystem::Behaviors(
                zmk::behaviors::Request {
                    request_type: Some(zmk::behaviors::request::RequestType::ListAllBehaviors(
                        true,
                    )),
                },
            ))
            .await?;
        match behavior_response(response)? {
            zmk::behaviors::response::ResponseType::ListAllBehaviors(list) => Ok(list.behaviors),
            other => Err(unexpected_behavior(&other)),
        }
    }

    pub async fn get_behavior_details(&self, behavior_id: u32) -> Result<Behavior, StudioError> {
        let response = self
            .read_request(zmk::studio::request::Subsystem::Behaviors(
                zmk::behaviors::Request {
                    request_type: Some(zmk::behaviors::request::RequestType::GetBehaviorDetails(
                        zmk::behaviors::GetBehaviorDetailsRequest { behavior_id },
                    )),
                },
            ))
            .await?;
        match behavior_response(response)? {
            zmk::behaviors::response::ResponseType::GetBehaviorDetails(details) => {
                Ok(convert_behavior(details))
            }
            other => Err(unexpected_behavior(&other)),
        }
    }

    /// Fetch every behavior advertised by the device, tolerating one missing
    /// entry rather than failing the whole catalog.
    pub async fn load_catalog(&self) -> Result<(Catalog, Vec<String>), StudioError> {
        let ids = self.list_behaviors().await?;
        let mut catalog = Catalog::default();
        let mut warnings = Vec::new();
        for id in ids {
            match self.get_behavior_details(id).await {
                Ok(behavior) => catalog.insert(behavior),
                Err(StudioError::UnlockRequired) => return Err(StudioError::UnlockRequired),
                Err(error) => warnings.push(format!("behavior {id}: {error}")),
            }
        }
        Ok((catalog, warnings))
    }

    pub async fn set_layer_binding(
        &self,
        layer_id: u32,
        position: usize,
        binding: Binding,
    ) -> Result<(), StudioError> {
        let behavior_id = i32::try_from(binding.behavior_id)
            .map_err(|_| StudioError::Protocol("behavior ID does not fit in i32".into()))?;
        let key_position = i32::try_from(position)
            .map_err(|_| StudioError::Protocol("key position does not fit in i32".into()))?;
        let response = self
            .request(zmk::studio::request::Subsystem::Keymap(
                zmk::keymap::Request {
                    request_type: Some(zmk::keymap::request::RequestType::SetLayerBinding(
                        zmk::keymap::SetLayerBindingRequest {
                            layer_id,
                            key_position,
                            binding: Some(zmk::keymap::BehaviorBinding {
                                behavior_id,
                                param1: binding.param1,
                                param2: binding.param2,
                            }),
                        },
                    )),
                },
            ))
            .await?;
        match keymap_response(response)? {
            zmk::keymap::response::ResponseType::SetLayerBinding(code) => {
                match zmk::keymap::SetLayerBindingResponse::try_from(code) {
                    Ok(zmk::keymap::SetLayerBindingResponse::SetLayerBindingRespOk) => {
                        self.set_unsaved(true).await;
                        Ok(())
                    }
                    Ok(other) => Err(StudioError::Device(format!("binding rejected ({other:?})"))),
                    Err(_) => Err(StudioError::Protocol(format!(
                        "unknown set_layer_binding response {code}"
                    ))),
                }
            }
            other => Err(unexpected_keymap(&other)),
        }
    }

    pub async fn save_changes(&self) -> Result<(), StudioError> {
        let response = self
            .request(zmk::studio::request::Subsystem::Keymap(
                zmk::keymap::Request {
                    request_type: Some(zmk::keymap::request::RequestType::SaveChanges(true)),
                },
            ))
            .await?;
        match keymap_response(response)? {
            zmk::keymap::response::ResponseType::SaveChanges(save) => match save.result {
                Some(zmk::keymap::save_changes_response::Result::Ok(true)) => {
                    self.set_unsaved(false).await;
                    Ok(())
                }
                Some(zmk::keymap::save_changes_response::Result::Ok(false)) => Err(
                    StudioError::Device("the keyboard did not save the keymap".into()),
                ),
                Some(zmk::keymap::save_changes_response::Result::Err(code)) => Err(
                    StudioError::Device(match zmk::keymap::SaveChangesErrorCode::try_from(code) {
                        Ok(zmk::keymap::SaveChangesErrorCode::SaveChangesErrNoSpace) => {
                            "the keyboard has no storage space left".to_string()
                        }
                        Ok(zmk::keymap::SaveChangesErrorCode::SaveChangesErrNotSupported) => {
                            "this firmware build does not persist keymap changes".to_string()
                        }
                        _ => "saving the keymap failed".to_string(),
                    }),
                ),
                None => Err(StudioError::Protocol("empty save response".into())),
            },
            other => Err(unexpected_keymap(&other)),
        }
    }

    pub async fn discard_changes(&self) -> Result<(), StudioError> {
        let response = self
            .request(zmk::studio::request::Subsystem::Keymap(
                zmk::keymap::Request {
                    request_type: Some(zmk::keymap::request::RequestType::DiscardChanges(true)),
                },
            ))
            .await?;
        match keymap_response(response)? {
            zmk::keymap::response::ResponseType::DiscardChanges(true) => {
                self.set_unsaved(false).await;
                Ok(())
            }
            zmk::keymap::response::ResponseType::DiscardChanges(false) => Err(StudioError::Device(
                "the keyboard could not discard the changes".into(),
            )),
            other => Err(unexpected_keymap(&other)),
        }
    }

    /// Restore the firmware's stock keymap and settings, discarding every
    /// saved Studio change.
    pub async fn reset_settings(&self) -> Result<(), StudioError> {
        let response = self
            .request(zmk::studio::request::Subsystem::Core(core_request(
                zmk::core::request::RequestType::ResetSettings(true),
            )))
            .await?;
        match core_response(response)? {
            zmk::core::response::ResponseType::ResetSettings(true) => {
                self.set_unsaved(false).await;
                Ok(())
            }
            zmk::core::response::ResponseType::ResetSettings(false) => Err(StudioError::Device(
                "the keyboard could not restore stock settings".into(),
            )),
            other => Err(unexpected_core(&other)),
        }
    }

    pub async fn add_layer(&self) -> Result<u32, StudioError> {
        let response = self
            .request(zmk::studio::request::Subsystem::Keymap(
                zmk::keymap::Request {
                    request_type: Some(zmk::keymap::request::RequestType::AddLayer(
                        zmk::keymap::AddLayerRequest {},
                    )),
                },
            ))
            .await?;
        match keymap_response(response)? {
            zmk::keymap::response::ResponseType::AddLayer(add) => match add.result {
                Some(zmk::keymap::add_layer_response::Result::Ok(details)) => {
                    self.set_unsaved(true).await;
                    Ok(details.index)
                }
                Some(zmk::keymap::add_layer_response::Result::Err(code)) => Err(
                    StudioError::Device(match zmk::keymap::AddLayerErrorCode::try_from(code) {
                        Ok(zmk::keymap::AddLayerErrorCode::AddLayerErrNoSpace) => {
                            "no free layer slots".to_string()
                        }
                        _ => "adding a layer failed".to_string(),
                    }),
                ),
                None => Err(StudioError::Protocol("empty add-layer response".into())),
            },
            other => Err(unexpected_keymap(&other)),
        }
    }

    pub async fn remove_layer(&self, index: usize) -> Result<(), StudioError> {
        let layer_index = u32::try_from(index)
            .map_err(|_| StudioError::Protocol("layer index does not fit in u32".into()))?;
        let response = self
            .request(zmk::studio::request::Subsystem::Keymap(
                zmk::keymap::Request {
                    request_type: Some(zmk::keymap::request::RequestType::RemoveLayer(
                        zmk::keymap::RemoveLayerRequest { layer_index },
                    )),
                },
            ))
            .await?;
        match keymap_response(response)? {
            zmk::keymap::response::ResponseType::RemoveLayer(remove) => match remove.result {
                Some(zmk::keymap::remove_layer_response::Result::Ok(_)) => {
                    self.set_unsaved(true).await;
                    Ok(())
                }
                Some(zmk::keymap::remove_layer_response::Result::Err(code)) => Err(
                    StudioError::Device(match zmk::keymap::RemoveLayerErrorCode::try_from(code) {
                        Ok(zmk::keymap::RemoveLayerErrorCode::RemoveLayerErrInvalidIndex) => {
                            "that layer cannot be removed".to_string()
                        }
                        _ => "removing the layer failed".to_string(),
                    }),
                ),
                None => Err(StudioError::Protocol("empty remove-layer response".into())),
            },
            other => Err(unexpected_keymap(&other)),
        }
    }

    pub async fn move_layer(
        &self,
        start_index: usize,
        dest_index: usize,
    ) -> Result<Keymap, StudioError> {
        let start_index = u32::try_from(start_index)
            .map_err(|_| StudioError::Protocol("layer index does not fit in u32".into()))?;
        let dest_index = u32::try_from(dest_index)
            .map_err(|_| StudioError::Protocol("layer index does not fit in u32".into()))?;
        let response = self
            .request(zmk::studio::request::Subsystem::Keymap(
                zmk::keymap::Request {
                    request_type: Some(zmk::keymap::request::RequestType::MoveLayer(
                        zmk::keymap::MoveLayerRequest {
                            start_index,
                            dest_index,
                        },
                    )),
                },
            ))
            .await?;
        match keymap_response(response)? {
            zmk::keymap::response::ResponseType::MoveLayer(moved) => match moved.result {
                Some(zmk::keymap::move_layer_response::Result::Ok(keymap)) => {
                    self.set_unsaved(true).await;
                    Ok(convert_keymap(keymap))
                }
                Some(zmk::keymap::move_layer_response::Result::Err(code)) => Err(
                    StudioError::Device(format!("moving the layer failed ({code})")),
                ),
                None => Err(StudioError::Protocol("empty move-layer response".into())),
            },
            other => Err(unexpected_keymap(&other)),
        }
    }

    pub async fn set_layer_props(&self, layer_id: u32, name: &str) -> Result<(), StudioError> {
        let response = self
            .request(zmk::studio::request::Subsystem::Keymap(
                zmk::keymap::Request {
                    request_type: Some(zmk::keymap::request::RequestType::SetLayerProps(
                        zmk::keymap::SetLayerPropsRequest {
                            layer_id,
                            name: name.to_string(),
                        },
                    )),
                },
            ))
            .await?;
        match keymap_response(response)? {
            zmk::keymap::response::ResponseType::SetLayerProps(code) => {
                match zmk::keymap::SetLayerPropsResponse::try_from(code) {
                    Ok(zmk::keymap::SetLayerPropsResponse::SetLayerPropsRespOk) => {
                        self.set_unsaved(true).await;
                        Ok(())
                    }
                    Ok(other) => Err(StudioError::Device(format!(
                        "renaming the layer failed ({other:?})"
                    ))),
                    Err(_) => Err(StudioError::Protocol(format!(
                        "unknown set_layer_props response {code}"
                    ))),
                }
            }
            other => Err(unexpected_keymap(&other)),
        }
    }

    pub async fn set_active_physical_layout(&self, index: usize) -> Result<Keymap, StudioError> {
        let index = u32::try_from(index)
            .map_err(|_| StudioError::Protocol("layout index does not fit in u32".into()))?;
        let response = self
            .request(zmk::studio::request::Subsystem::Keymap(
                zmk::keymap::Request {
                    request_type: Some(zmk::keymap::request::RequestType::SetActivePhysicalLayout(
                        index,
                    )),
                },
            ))
            .await?;
        match keymap_response(response)? {
            zmk::keymap::response::ResponseType::SetActivePhysicalLayout(result) => {
                match result.result {
                    Some(zmk::keymap::set_active_physical_layout_response::Result::Ok(keymap)) => {
                        self.set_unsaved(true).await;
                        Ok(convert_keymap(keymap))
                    }
                    Some(zmk::keymap::set_active_physical_layout_response::Result::Err(code)) => {
                        Err(StudioError::Device(format!(
                            "selecting the physical layout failed ({code})"
                        )))
                    }
                    None => Err(StudioError::Protocol("empty layout response".into())),
                }
            }
            other => Err(unexpected_keymap(&other)),
        }
    }

    async fn set_lock_state(&self, lock: LockState) {
        let changed = {
            let mut state = self.inner.state.write().await;
            let changed = state.lock != Some(lock);
            state.lock = Some(lock);
            changed
        };
        if changed {
            let _ = self.inner.events.send(StudioEvent::LockStateChanged(lock));
        }
    }

    async fn set_unsaved(&self, unsaved: bool) {
        let changed = {
            let mut state = self.inner.state.write().await;
            let changed = state.unsaved != Some(unsaved);
            state.unsaved = Some(unsaved);
            changed
        };
        if changed {
            let _ = self
                .inner
                .events
                .send(StudioEvent::UnsavedChangesChanged(unsaved));
        }
    }
}

async fn read_loop(
    mut frames: tokio::sync::mpsc::UnboundedReceiver<crate::transport::FrameResult>,
    inner: std::sync::Weak<Inner>,
) {
    while let Some(frame) = frames.recv().await {
        let Some(inner_ref) = inner.upgrade() else {
            return;
        };
        match frame {
            Ok(payload) => {
                let Ok(response) = zmk::studio::Response::decode(payload.as_slice()) else {
                    continue;
                };
                match response.r#type {
                    Some(zmk::studio::response::Type::RequestResponse(request_response)) => {
                        let sender = inner_ref
                            .pending
                            .lock()
                            .await
                            .remove(&request_response.request_id);
                        if let Some(sender) = sender {
                            let _ = sender.send(request_response);
                        }
                    }
                    Some(zmk::studio::response::Type::Notification(notification)) => {
                        handle_notification(notification, &inner_ref).await;
                    }
                    None => {}
                }
            }
            Err(error) => {
                let mut state = inner_ref.state.write().await;
                state.connected = false;
                drop(state);
                let _ = inner_ref.events.send(StudioEvent::Disconnected(error));
                return;
            }
        }
    }

    let Some(inner) = inner.upgrade() else {
        return;
    };
    let mut state = inner.state.write().await;
    state.connected = false;
    drop(state);
    let _ = inner
        .events
        .send(StudioEvent::Disconnected("connection closed".into()));
}

async fn handle_notification(notification: zmk::studio::Notification, inner: &Inner) {
    match notification.subsystem {
        Some(zmk::studio::notification::Subsystem::Core(core)) => {
            if let Some(zmk::core::notification::NotificationType::LockStateChanged(value)) =
                core.notification_type
            {
                if let Some(lock) = proto_lock_state(value) {
                    let changed = {
                        let mut state = inner.state.write().await;
                        let changed = state.lock != Some(lock);
                        state.lock = Some(lock);
                        changed
                    };
                    if changed {
                        let _ = inner.events.send(StudioEvent::LockStateChanged(lock));
                    }
                }
            }
        }
        Some(zmk::studio::notification::Subsystem::Keymap(keymap)) => {
            if let Some(zmk::keymap::notification::NotificationType::UnsavedChangesStatusChanged(
                unsaved,
            )) = keymap.notification_type
            {
                let changed = {
                    let mut state = inner.state.write().await;
                    let changed = state.unsaved != Some(unsaved);
                    state.unsaved = Some(unsaved);
                    changed
                };
                if changed {
                    let _ = inner
                        .events
                        .send(StudioEvent::UnsavedChangesChanged(unsaved));
                }
            }
        }
        None => {}
    }
}

fn core_request(request_type: zmk::core::request::RequestType) -> zmk::core::Request {
    zmk::core::Request {
        request_type: Some(request_type),
    }
}

fn meta_error(meta: &zmk::meta::Response) -> StudioError {
    match meta.response_type {
        Some(zmk::meta::response::ResponseType::NoResponse(_)) => StudioError::RpcNotFound,
        Some(zmk::meta::response::ResponseType::SimpleError(code)) => {
            match zmk::meta::ErrorConditions::try_from(code) {
                Ok(zmk::meta::ErrorConditions::UnlockRequired) => StudioError::UnlockRequired,
                Ok(zmk::meta::ErrorConditions::RpcNotFound) => StudioError::RpcNotFound,
                Ok(other) => StudioError::Device(format!("{other:?}")),
                Err(_) => StudioError::Protocol(format!("unknown meta error {code}")),
            }
        }
        None => StudioError::Protocol("empty meta response".into()),
    }
}

fn core_response(
    response: zmk::studio::RequestResponse,
) -> Result<zmk::core::response::ResponseType, StudioError> {
    match response.subsystem {
        Some(zmk::studio::request_response::Subsystem::Core(core)) => core
            .response_type
            .ok_or_else(|| StudioError::Protocol("empty core response".into())),
        Some(other) => Err(unexpected_subsystem(&other)),
        None => Err(StudioError::Protocol("empty response".into())),
    }
}

fn keymap_response(
    response: zmk::studio::RequestResponse,
) -> Result<zmk::keymap::response::ResponseType, StudioError> {
    match response.subsystem {
        Some(zmk::studio::request_response::Subsystem::Keymap(keymap)) => keymap
            .response_type
            .ok_or_else(|| StudioError::Protocol("empty keymap response".into())),
        Some(other) => Err(unexpected_subsystem(&other)),
        None => Err(StudioError::Protocol("empty response".into())),
    }
}

fn behavior_response(
    response: zmk::studio::RequestResponse,
) -> Result<zmk::behaviors::response::ResponseType, StudioError> {
    match response.subsystem {
        Some(zmk::studio::request_response::Subsystem::Behaviors(behaviors)) => behaviors
            .response_type
            .ok_or_else(|| StudioError::Protocol("empty behaviors response".into())),
        Some(other) => Err(unexpected_subsystem(&other)),
        None => Err(StudioError::Protocol("empty response".into())),
    }
}

fn unexpected_subsystem(subsystem: &zmk::studio::request_response::Subsystem) -> StudioError {
    let name = match subsystem {
        zmk::studio::request_response::Subsystem::Meta(_) => "meta",
        zmk::studio::request_response::Subsystem::Core(_) => "core",
        zmk::studio::request_response::Subsystem::Behaviors(_) => "behaviors",
        zmk::studio::request_response::Subsystem::Keymap(_) => "keymap",
    };
    StudioError::Unexpected(format!(
        "received a {name} response for a different request"
    ))
}

fn unexpected_core(response: &zmk::core::response::ResponseType) -> StudioError {
    StudioError::Unexpected(format!("unexpected core response: {response:?}"))
}

fn unexpected_keymap(response: &zmk::keymap::response::ResponseType) -> StudioError {
    StudioError::Unexpected(format!("unexpected keymap response: {response:?}"))
}

fn unexpected_behavior(response: &zmk::behaviors::response::ResponseType) -> StudioError {
    StudioError::Unexpected(format!("unexpected behavior response: {response:?}"))
}

fn proto_lock_state(value: i32) -> Option<LockState> {
    match zmk::core::LockState::try_from(value) {
        Ok(zmk::core::LockState::ZmkStudioCoreLockStateLocked) => Some(LockState::Locked),
        Ok(zmk::core::LockState::ZmkStudioCoreLockStateUnlocked) => Some(LockState::Unlocked),
        Err(_) => None,
    }
}

fn convert_keymap(keymap: zmk::keymap::Keymap) -> Keymap {
    Keymap {
        layers: keymap
            .layers
            .into_iter()
            .map(|layer| Layer {
                id: layer.id,
                name: layer.name,
                bindings: layer
                    .bindings
                    .into_iter()
                    .map(|binding| Binding {
                        behavior_id: binding.behavior_id.max(0) as u32,
                        param1: binding.param1,
                        param2: binding.param2,
                    })
                    .collect(),
            })
            .collect(),
        available_layers: keymap.available_layers,
        max_layer_name_length: keymap.max_layer_name_length as usize,
    }
}

fn convert_layouts(layouts: zmk::keymap::PhysicalLayouts) -> PhysicalLayouts {
    PhysicalLayouts {
        active_index: layouts.active_layout_index as usize,
        layouts: layouts
            .layouts
            .into_iter()
            .map(|layout| PhysicalLayout {
                name: layout.name,
                keys: layout
                    .keys
                    .into_iter()
                    .map(|key| {
                        KeyGeometry::from_centi_units(
                            key.x, key.y, key.width, key.height, key.r, key.rx, key.ry,
                        )
                    })
                    .collect(),
            })
            .collect(),
    }
}

fn convert_behavior(details: zmk::behaviors::GetBehaviorDetailsResponse) -> Behavior {
    Behavior {
        id: details.id,
        display_name: details.display_name,
        metadata: details
            .metadata
            .into_iter()
            .map(|set| ParamSet {
                param1: set.param1.into_iter().map(convert_param).collect(),
                param2: set.param2.into_iter().map(convert_param).collect(),
            })
            .collect(),
    }
}

fn convert_param(param: zmk::behaviors::BehaviorParameterValueDescription) -> ParamDesc {
    let kind = match param.value_type {
        Some(zmk::behaviors::behavior_parameter_value_description::ValueType::Nil(_)) => {
            ParamKind::Nil
        }
        Some(zmk::behaviors::behavior_parameter_value_description::ValueType::Constant(value)) => {
            ParamKind::Constant(value)
        }
        Some(zmk::behaviors::behavior_parameter_value_description::ValueType::Range(range)) => {
            ParamKind::Range {
                min: range.min,
                max: range.max,
            }
        }
        Some(zmk::behaviors::behavior_parameter_value_description::ValueType::HidUsage(usage)) => {
            ParamKind::HidUsage {
                keyboard_max: usage.keyboard_max,
                consumer_max: usage.consumer_max,
            }
        }
        Some(zmk::behaviors::behavior_parameter_value_description::ValueType::LayerId(_)) => {
            ParamKind::LayerId
        }
        None => ParamKind::Nil,
    };
    ParamDesc {
        name: param.name,
        kind,
    }
}
