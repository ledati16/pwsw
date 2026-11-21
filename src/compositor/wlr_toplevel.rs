//! `wlr-foreign-toplevel-management` protocol backend
//!
//! Supported by Sway, Hyprland, Wayfire, River, labwc, dwl, Niri and other wlroots-style
//! compositors. Protocol events are translated into calls on the shared
//! [`WindowTracker`]; window control requests are never used.

use color_eyre::eyre::{Context, Result};
use tracing::error;
use wayland_client::globals::GlobalList;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_handle_v1::{
    self, ZwlrForeignToplevelHandleV1,
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_manager_v1::{
    self, ZwlrForeignToplevelManagerV1,
};

use super::tracker::WindowTracker;

/// Wayland interface name of the global this backend binds
pub(super) const INTERFACE: &str = "zwlr_foreign_toplevel_manager_v1";

/// Bind the `zwlr_foreign_toplevel_manager_v1` global (versions 1-3)
///
/// # Errors
/// Returns an error if the global is missing or advertised at an unsupported version.
pub(super) fn bind(
    globals: &GlobalList,
    qh: &QueueHandle<WindowTracker>,
) -> Result<ZwlrForeignToplevelManagerV1> {
    globals
        .bind(qh, 1..=3, ())
        .context("Failed to bind `zwlr_foreign_toplevel_manager_v1`")
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for WindowTracker {
    fn event(
        state: &mut Self,
        _proxy: &ZwlrForeignToplevelManagerV1,
        event: zwlr_foreign_toplevel_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use zwlr_foreign_toplevel_manager_v1::Event;

        match event {
            Event::Toplevel { toplevel } => state.handle_created(toplevel.id()),
            Event::Finished => {
                // `finished` is a destructor event: the manager is already destroyed
                error!("Compositor finished the `zwlr_foreign_toplevel_manager_v1` object");
                state.set_finished();
            }
            _ => {}
        }
    }

    wayland_client::event_created_child!(WindowTracker, ZwlrForeignToplevelManagerV1, [
        zwlr_foreign_toplevel_manager_v1::EVT_TOPLEVEL_OPCODE => (ZwlrForeignToplevelHandleV1, ())
    ]);
}

impl Dispatch<ZwlrForeignToplevelHandleV1, ()> for WindowTracker {
    fn event(
        state: &mut Self,
        proxy: &ZwlrForeignToplevelHandleV1,
        event: zwlr_foreign_toplevel_handle_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use zwlr_foreign_toplevel_handle_v1::Event;

        let key = proxy.id();
        match event {
            Event::Title { title } => state.set_title(&key, title),
            Event::AppId { app_id } => state.set_app_id(&key, app_id),
            // A `done` after only `state`/`output_*`/`parent` changes emits nothing:
            // the tracker compares `app_id` and title with what it last reported.
            Event::Done => state.done(&key),
            Event::Closed => {
                state.closed(&key);
                // The protocol requires the client to destroy the handle after `closed`;
                // `wayland-client` does not destroy proxies on drop.
                proxy.destroy();
            }
            // `state`, `output_enter`, `output_leave`, `parent`: not needed
            _ => {}
        }
    }
}
