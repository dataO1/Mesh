//! Browser and USB message handler
//!
//! Handles collection browser navigation, track selection, and USB device events.

use iced::Task;

use mesh_core::usb::UsbMessage as UsbMsg;
use mesh_widgets::{TRACK_ROW_HEIGHT, TRACK_TABLE_SCROLLABLE_ID};
use crate::ui::app::MeshApp;
use crate::ui::collection_browser::CollectionBrowserMessage;
use crate::ui::message::Message;

/// Handle collection browser messages
pub fn handle_browser(app: &mut MeshApp, browser_msg: CollectionBrowserMessage) -> Task<Message> {
    // Check if this is a scroll message (for auto-scroll after)
    let is_scroll = matches!(browser_msg, CollectionBrowserMessage::ScrollBy(_));

    // Handle collection browser message and check if we need to load a track
    if let Some((deck_idx, path)) = app.collection_browser.handle_message(browser_msg) {
        // Convert to LoadTrack message
        let path_str = path.to_string_lossy().to_string();
        return app.update(Message::LoadTrack(deck_idx, path_str));
    }

    // Sync domain layer with collection browser's active storage
    // This ensures metadata is loaded from the correct database (local or USB)
    if let Some((usb_idx, collection_path)) = app.collection_browser.get_active_usb_info() {
        // Browsing USB - switch domain to USB if not already
        if !app.domain.is_browsing_usb() {
            if let Err(e) = app.domain.switch_to_usb(usb_idx, &collection_path) {
                log::error!("Failed to switch domain to USB: {}", e);
            }
        }
    } else if app.domain.is_browsing_usb() {
        // Browsing local - switch domain back if it was on USB
        app.domain.switch_to_local();
    }

    // If it was a scroll, create a Task to auto-scroll the track list
    if is_scroll {
        if let Some(selected_idx) = app.collection_browser.get_selected_index() {
            // Calculate scroll offset to keep selection centered in view
            // Assume ~10 visible rows; center selection with some margin
            let visible_rows = 10.0_f32;
            let center_offset = (visible_rows / 2.0 - 1.0) * TRACK_ROW_HEIGHT;
            let target_y = (selected_idx as f32 * TRACK_ROW_HEIGHT - center_offset)
                .max(0.0);

            // Create scroll operation
            use iced::widget::scrollable;
            let offset = scrollable::AbsoluteOffset { x: 0.0, y: target_y };
            let scroll_id = TRACK_TABLE_SCROLLABLE_ID.clone();

            // Use iced's widget operation system to scroll
            return iced::advanced::widget::operate(
                iced::advanced::widget::operation::scrollable::scroll_to(
                    scroll_id.into(),
                    offset.into(),
                )
            );
        }
    }

    Task::none()
}

/// Handle USB device messages
pub fn handle_usb(app: &mut MeshApp, usb_msg: UsbMsg) -> Task<Message> {
    match usb_msg {
        UsbMsg::DevicesRefreshed(devices) => {
            log::info!("USB: {} devices detected", devices.len());
            app.collection_browser.update_usb_devices(devices.clone());
            // Initialize storages for mounted devices and trigger metadata preload
            for device in &devices {
                if device.mount_point.is_some() && device.has_mesh_collection {
                    app.collection_browser.init_usb_storage(device);
                    // Trigger background metadata preload for instant browsing
                    let _ = app.domain.send_usb_command(
                        mesh_core::usb::UsbCommand::PreloadMetadata {
                            device_path: device.device_path.clone(),
                        }
                    );
                }
            }
        }
        UsbMsg::DeviceConnected(device) => {
            app.collection_browser.add_usb_device(device.clone());
            if device.mount_point.is_some() && device.has_mesh_collection {
                app.collection_browser.init_usb_storage(&device);
                // Trigger background metadata preload for instant browsing
                let _ = app.domain.send_usb_command(
                    mesh_core::usb::UsbCommand::PreloadMetadata {
                        device_path: device.device_path.clone(),
                    }
                );
            }
            app.status = format!("USB: {} connected", device.label);
        }
        UsbMsg::DeviceDisconnected { device_path } => {
            app.collection_browser.remove_usb_device(&device_path);
            app.status = "USB: Device disconnected".to_string();
        }
        UsbMsg::MountComplete { result } => {
            match result {
                Ok(device) => {
                    // Update device in browser and init storage if it has mesh collection
                    app.collection_browser.add_usb_device(device.clone());
                    if device.has_mesh_collection {
                        app.collection_browser.init_usb_storage(&device);
                        // Trigger background metadata preload for instant browsing
                        let _ = app.domain.send_usb_command(
                            mesh_core::usb::UsbCommand::PreloadMetadata {
                                device_path: device.device_path.clone(),
                            }
                        );
                    }
                }
                Err(e) => {
                    log::warn!("USB mount failed: {}", e);
                }
            }
        }
        UsbMsg::MetadataPreloaded { device_path, metadata } => {
            // Metadata now comes from USB's mesh.db, no caching needed
            log::debug!("USB: Metadata preload complete for {} ({} tracks)",
                device_path.display(), metadata.len());
        }
        UsbMsg::MetadataPreloadProgress { device_path, loaded, total } => {
            log::debug!("USB: Preloading metadata {}/{} from {}",
                loaded, total, device_path.display());
        }
        _ => {
            // Ignore export-related messages in mesh-player (read-only)
        }
    }
    Task::none()
}
