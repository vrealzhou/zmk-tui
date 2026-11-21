//! End-to-end tests of the RPC client against the in-process simulator.

use std::time::Duration;

use zmk_tui::client::{StudioClient, StudioError, StudioEvent};
use zmk_tui::model::{Binding, LockState};
use zmk_tui::simulator::{SimulatedBoard, Simulator};

async fn connected(locked: bool) -> (Simulator, StudioClient) {
    let (simulator, connection) = Simulator::start(SimulatedBoard::Corne, locked);
    let client = StudioClient::start(connection);
    (simulator, client)
}

async fn wait_for_unlock(rx: &mut tokio::sync::broadcast::Receiver<StudioEvent>) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match rx.recv().await {
                Ok(StudioEvent::LockStateChanged(LockState::Unlocked)) => return,
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    panic!("event channel closed")
                }
            }
        }
    })
    .await
    .expect("unlock notification");
}

#[tokio::test]
async fn reads_device_info_and_lock_state() {
    let (_sim, client) = connected(false).await;
    let info = client.get_device_info().await.expect("device info");
    assert_eq!(info.name, "Corne (simulator)");
    assert_eq!(info.serial_number, b"SIM-0001");

    assert_eq!(client.get_lock_state().await.unwrap(), LockState::Unlocked);
    assert_eq!(client.cached_lock_state().await, Some(LockState::Unlocked));
    assert!(client.is_connected().await);
}

#[tokio::test]
async fn locked_device_rejects_edits_until_unlocked() {
    let (sim, client) = connected(true).await;
    assert_eq!(client.get_lock_state().await.unwrap(), LockState::Locked);

    // Unsecured calls still work.
    assert_eq!(client.list_behaviors().await.unwrap().len(), 12);

    // Secured calls report the lock.
    assert!(matches!(
        client.get_keymap().await,
        Err(StudioError::UnlockRequired)
    ));
    assert!(matches!(
        client.get_behavior_details(1001).await,
        Err(StudioError::UnlockRequired)
    ));
    assert!(matches!(
        client
            .set_layer_binding(
                0,
                0,
                Binding {
                    behavior_id: 1001,
                    param1: 0x00070004,
                    param2: 0
                }
            )
            .await,
        Err(StudioError::UnlockRequired)
    ));

    let mut events = client.subscribe();
    sim.unlock();
    wait_for_unlock(&mut events).await;

    assert_eq!(client.get_lock_state().await.unwrap(), LockState::Unlocked);
    let keymap = client.get_keymap().await.expect("keymap after unlock");
    assert_eq!(keymap.layers.len(), 5);
}

#[tokio::test]
async fn reads_keymap_layouts_and_catalog() {
    let (_sim, client) = connected(false).await;

    let keymap = client.get_keymap().await.expect("keymap");
    assert_eq!(keymap.layers.len(), 5);
    assert_eq!(keymap.layers[0].name, "Base");
    assert_eq!(keymap.layers[3].name, "Adjust");
    assert_eq!(keymap.layers[4].name, "Media");
    assert_eq!(keymap.layers[0].bindings.len(), 42);
    assert_eq!(keymap.available_layers, 3);
    assert_eq!(keymap.layers[0].bindings[0].param1, 0x00070029); // ESCAPE

    let layouts = client.get_physical_layouts().await.expect("layouts");
    assert_eq!(layouts.active_index, 0);
    assert_eq!(layouts.layouts.len(), 2);
    assert_eq!(layouts.layouts[0].keys.len(), 42);
    assert_eq!(layouts.layouts[0].keys[1].x, 1.0);
    assert_eq!(layouts.layouts[1].name, "Corne wide");

    let (catalog, warnings) = client.load_catalog().await.expect("catalog");
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(catalog.len(), 12);
    let key_press = catalog.find_by_name("Key Press").expect("Key Press");
    assert!(key_press.has_params());
    let mod_tap = catalog.find_by_name("Mod-Tap").expect("Mod-Tap");
    assert_eq!(mod_tap.primary_param_set().unwrap().param2.len(), 1);
    assert!(catalog.find_by_name("Transparent").is_some());
}

#[tokio::test]
async fn edits_save_and_discard() {
    let (_sim, client) = connected(false).await;
    let mut events = client.subscribe();

    let keymap = client.get_keymap().await.unwrap();
    assert!(!client.check_unsaved_changes().await.unwrap());

    let binding = Binding {
        behavior_id: 1001,
        param1: 0x00070004, // A
        param2: 0,
    };
    client
        .set_layer_binding(keymap.layers[0].id, 0, binding)
        .await
        .expect("set binding");

    let changed = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Ok(StudioEvent::UnsavedChangesChanged(true)) = events.recv().await {
                return;
            }
        }
    })
    .await;
    assert!(changed.is_ok(), "expected an unsaved-changes notification");
    assert_eq!(client.cached_unsaved_changes().await, Some(true));
    assert!(client.check_unsaved_changes().await.unwrap());

    let updated = client.get_keymap().await.unwrap();
    assert_eq!(updated.layers[0].bindings[0], binding);

    client.save_changes().await.expect("save");
    assert!(!client.check_unsaved_changes().await.unwrap());

    // A change followed by discard restores the saved value.
    let other = Binding {
        behavior_id: 1001,
        param1: 0x00070005,
        param2: 0,
    };
    client
        .set_layer_binding(keymap.layers[0].id, 0, other)
        .await
        .unwrap();
    assert_eq!(
        client.get_keymap().await.unwrap().layers[0].bindings[0],
        other
    );
    client.discard_changes().await.expect("discard");
    assert_eq!(
        client.get_keymap().await.unwrap().layers[0].bindings[0],
        binding
    );
}

#[tokio::test]
async fn manages_layers() {
    let (_sim, client) = connected(false).await;

    let index = client.add_layer().await.expect("add layer");
    assert_eq!(index, 5);
    let keymap = client.get_keymap().await.unwrap();
    assert_eq!(keymap.layers.len(), 6);
    assert_eq!(keymap.layers[5].bindings.len(), 42);

    let new_layer_id = keymap.layers[5].id;
    client
        .set_layer_props(new_layer_id, "Fn")
        .await
        .expect("rename layer");
    assert_eq!(client.get_keymap().await.unwrap().layers[5].name, "Fn");

    let moved = client.move_layer(5, 1).await.expect("move layer");
    assert_eq!(moved.layers[1].name, "Fn");
    assert_eq!(moved.layers[0].name, "Base");

    client.remove_layer(1).await.expect("remove layer");
    let keymap = client.get_keymap().await.unwrap();
    assert_eq!(keymap.layers.len(), 5);
    assert_eq!(keymap.layers[0].name, "Base");

    assert!(matches!(
        client.remove_layer(99).await,
        Err(StudioError::Device(_))
    ));
    assert!(matches!(
        client.set_layer_binding(9999, 0, Binding::default()).await,
        Err(StudioError::Device(_))
    ));
}

#[tokio::test]
async fn sofle_reports_its_sixty_key_layout() {
    let (simulator, connection) = Simulator::start(SimulatedBoard::Sofle, false);
    let client = StudioClient::start(connection);

    let info = client.get_device_info().await.expect("device info");
    assert_eq!(info.name, "Sofle (simulator)");

    let keymap = client.get_keymap().await.expect("keymap");
    assert_eq!(keymap.layers.len(), 5);
    assert_eq!(keymap.layers[0].bindings.len(), 60);
    assert_eq!(keymap.layers[3].name, "Adjust");
    assert_eq!(keymap.layers[4].name, "Media");

    let layouts = client.get_physical_layouts().await.expect("layouts");
    assert_eq!(layouts.layouts.len(), 1);
    assert_eq!(layouts.layouts[0].name, "Sofle");
    assert_eq!(layouts.layouts[0].keys.len(), 60);
    // Column stagger: the outer first column sits lower than the middle one.
    assert!(layouts.layouts[0].keys[0].y > layouts.layouts[0].keys[3].y);
    // The encoder push buttons sit in the middle of the fourth row.
    assert_eq!(layouts.layouts[0].keys[42].x, 6.0);
    assert_eq!(layouts.layouts[0].keys[43].x, 8.0);
    // Ten thumb positions start at x=1.75.
    assert_eq!(layouts.layouts[0].keys[50].x, 1.75);
    assert_eq!(layouts.layouts[0].keys[59].x, 12.25);

    let (catalog, warnings) = client.load_catalog().await.expect("catalog");
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(catalog.len(), 12);

    // Editing the last thumb key works without any board-specific code.
    let gui = zmk_tui::keycodes::from_name("RGUI").unwrap().value;
    client
        .set_layer_binding(
            keymap.layers[0].id,
            59,
            Binding {
                behavior_id: 1001,
                param1: gui,
                param2: 0,
            },
        )
        .await
        .expect("set binding");
    assert_eq!(
        client.get_keymap().await.unwrap().layers[0].bindings[59].param1,
        gui
    );

    drop(simulator);
}

#[tokio::test]
async fn reports_disconnects() {
    let (sim, client) = connected(false).await;
    let mut events = client.subscribe();
    sim.disconnect();

    let disconnected = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(StudioEvent::Disconnected(reason)) = events.recv().await {
                return reason;
            }
        }
    })
    .await
    .expect("disconnect event");
    assert!(!disconnected.is_empty());
    assert!(!client.is_connected().await);

    assert!(client.get_device_info().await.is_err());
}

#[tokio::test]
async fn simulator_can_be_relocked() {
    let (sim, client) = connected(false).await;
    let mut events = client.subscribe();
    sim.lock();

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(StudioEvent::LockStateChanged(LockState::Locked)) = events.recv().await {
                return;
            }
        }
    })
    .await
    .expect("lock notification");

    assert!(matches!(
        client.get_keymap().await,
        Err(StudioError::UnlockRequired)
    ));
}
