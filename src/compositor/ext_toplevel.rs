//! `ext-foreign-toplevel-list-v1` protocol backend
//!
//! The standard protocol for listing toplevel windows. It provides read-only access to
//! window metadata (title, `app_id`). Protocol events are translated into calls on the
//! shared [`WindowTracker`].

use color_eyre::eyre::{Context, Result};
use tracing::error;
use wayland_client::globals::GlobalList;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_handle_v1::{
    self, ExtForeignToplevelHandleV1,
};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_list_v1::{
    self, ExtForeignToplevelListV1,
};

use super::tracker::WindowTracker;

/// Wayland interface name of the global this backend binds
pub(super) const INTERFACE: &str = "ext_foreign_toplevel_list_v1";

/// Bind the `ext_foreign_toplevel_list_v1` global
///
/// # Errors
/// Returns an error if the global is missing or advertised at an unsupported version.
pub(super) fn bind(
    globals: &GlobalList,
    qh: &QueueHandle<WindowTracker>,
) -> Result<ExtForeignToplevelListV1> {
    globals
        .bind(qh, 1..=1, ())
        .context("Failed to bind `ext_foreign_toplevel_list_v1`")
}

impl Dispatch<ExtForeignToplevelListV1, ()> for WindowTracker {
    fn event(
        state: &mut Self,
        proxy: &ExtForeignToplevelListV1,
        event: ext_foreign_toplevel_list_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use ext_foreign_toplevel_list_v1::Event;

        match event {
            Event::Toplevel { toplevel } => state.handle_created(toplevel.id()),
            Event::Finished => {
                error!("Compositor finished the `ext_foreign_toplevel_list_v1` object");
                // The protocol asks the client to destroy the list after `finished`
                proxy.destroy();
                state.set_finished();
            }
            _ => {}
        }
    }

    wayland_client::event_created_child!(WindowTracker, ExtForeignToplevelListV1, [
        ext_foreign_toplevel_list_v1::EVT_TOPLEVEL_OPCODE => (ExtForeignToplevelHandleV1, ())
    ]);
}

impl Dispatch<ExtForeignToplevelHandleV1, ()> for WindowTracker {
    fn event(
        state: &mut Self,
        proxy: &ExtForeignToplevelHandleV1,
        event: ext_foreign_toplevel_handle_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use ext_foreign_toplevel_handle_v1::Event;

        let key = proxy.id();
        match event {
            Event::Title { title } => state.set_title(&key, title),
            Event::AppId { app_id } => state.set_app_id(&key, app_id),
            Event::Done => state.done(&key),
            Event::Closed => {
                state.closed(&key);
                // The protocol requires the client to destroy the handle after `closed`;
                // `wayland-client` does not destroy proxies on drop.
                proxy.destroy();
            }
            // `identifier` (and future events) are not needed
            _ => {}
        }
    }
}
