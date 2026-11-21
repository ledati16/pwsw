//! `wl_output` tracking: which monitor (connector name) a `wl_output` object is
//!
//! Only bound for the `wlr` backend, whose toplevel handles report `output_enter` /
//! `output_leave` with `wl_output` objects; `ext-foreign-toplevel-list-v1` carries no
//! placement information. The registry `global` / `global_remove` events (hotplug) are
//! handled in the parent module; this module binds one global and turns its events into
//! calls on the [`WindowTracker`].

use tracing::debug;
use wayland_client::protocol::{wl_output, wl_registry};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};

use super::tracker::WindowTracker;

/// Wayland interface name of the output globals
pub(super) const INTERFACE: &str = "wl_output";

/// Highest `wl_output` version we bind: 4 added the `name` event (connector name)
const MAX_VERSION: u32 = 4;

/// First `wl_output` version with the `name` event
const NAME_SINCE: u32 = 4;

/// First `wl_output` version with the `release` request
const RELEASE_SINCE: u32 = 3;

/// Bind the `wl_output` global `global` (advertised at `version`)
///
/// The proxy's user data is the global name, which identifies the output in
/// `global_remove` and in the fallback label of an output without a connector name.
pub(super) fn bind(
    registry: &wl_registry::WlRegistry,
    global: u32,
    version: u32,
    qh: &QueueHandle<WindowTracker>,
) -> wl_output::WlOutput {
    let version = version.clamp(1, MAX_VERSION);
    if version < NAME_SINCE {
        debug!(
            "`wl_output` global {global} is version {version} (< {NAME_SINCE}): no connector \
             name, reporting it as `{}`",
            fallback_label(global)
        );
    }
    registry.bind::<wl_output::WlOutput, _, _>(global, version, qh, global)
}

/// Label of an output whose connector name is not known (yet, or ever: `wl_output` < 4)
pub(super) fn fallback_label(global: u32) -> String {
    format!("output-{global}")
}

/// Destroy our `wl_output` object after its global was removed (when the version allows)
pub(super) fn release(output: &wl_output::WlOutput) {
    if output.version() >= RELEASE_SINCE {
        output.release();
    }
}

impl Dispatch<wl_output::WlOutput, u32> for WindowTracker {
    fn event(
        state: &mut Self,
        proxy: &wl_output::WlOutput,
        event: wl_output::Event,
        _global: &u32,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // `geometry`, `mode`, `scale`, `description`, `done`: not needed. The protocol
        // sends `name` once, right after the bind and before any toplevel `output_enter`
        // that refers to this object.
        if let wl_output::Event::Name { name } = event {
            state.output_named(&proxy.id(), name);
        }
    }
}
