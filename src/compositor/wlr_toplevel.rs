//! `wlr-foreign-toplevel-management` protocol backend
//!
//! Supported by Sway, Hyprland, Wayfire, River, labwc, dwl, Niri and other wlroots-style
//! compositors. Protocol events are translated into calls on the shared
//! [`WindowTracker`]; window control requests are never used.
//!
//! Besides `app_id` and title, this backend reports window placement: the outputs a
//! toplevel is on (`output_enter` / `output_leave`) and its `minimized` state flag.

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

/// `zwlr_foreign_toplevel_handle_v1.state` value of the `minimized` flag
const STATE_MINIMIZED: u32 = 1;

/// Whether a `state` event's array (native-endian `u32` values) contains `minimized`
fn is_minimized(state: &[u8]) -> bool {
    state
        .as_chunks::<4>()
        .0
        .iter()
        .any(|bytes| u32::from_ne_bytes(*bytes) == STATE_MINIMIZED)
}

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
            Event::OutputEnter { output } => state.output_entered(&key, output.id()),
            Event::OutputLeave { output } => state.output_left(&key, &output.id()),
            Event::State { state: flags } => state.set_minimized(&key, is_minimized(&flags)),
            // A `done` after only `parent` or other state flag (focus, maximised) changes
            // emits nothing: the tracker compares with what it last reported.
            Event::Done => state.done(&key),
            Event::Closed => {
                state.closed(&key);
                // The protocol requires the client to destroy the handle after `closed`;
                // `wayland-client` does not destroy proxies on drop.
                proxy.destroy();
            }
            // `parent`: not needed
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flags(values: &[u32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_ne_bytes()).collect()
    }

    #[test]
    fn test_is_minimized() {
        assert!(!is_minimized(&[]));
        assert!(!is_minimized(&flags(&[0, 2, 3])));
        assert!(is_minimized(&flags(&[2, STATE_MINIMIZED])));
        // A truncated trailing value is ignored
        assert!(!is_minimized(&[1, 0]));
    }
}
