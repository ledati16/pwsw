//! Integration tests for Wayland compositor backends using a mock server.
//!
//! Each test runs its own mock compositor on a private socket, so tests are independent
//! and run in parallel. Synchronisation uses notices from the mock (bind, handle
//! creation/destruction, output binds) and event receipt with timeouts, never fixed
//! sleeps.
//!
//! The mock can advertise `wl_output` globals (with connector names from version 4) and
//! sends `output_enter` / `output_leave` / `state` on `wlr` handles, for the
//! `WindowEvent::Placement` tests.

use std::collections::HashMap;
use std::sync::mpsc as std_mpsc;
use std::thread;
use std::time::{Duration, Instant};

use pretty_assertions::assert_eq;
use pwsw::compositor::{
    Backend, Capabilities, ProtocolPreference, WindowEvent, spawn_compositor_thread_on,
    spawn_compositor_thread_on_with_preference,
};
use rstest::rstest;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::error::TryRecvError;
use wayland_protocols::ext::foreign_toplevel_list::v1::server::ext_foreign_toplevel_handle_v1::{
    self, ExtForeignToplevelHandleV1,
};
use wayland_protocols::ext::foreign_toplevel_list::v1::server::ext_foreign_toplevel_list_v1::{
    self, ExtForeignToplevelListV1,
};
use wayland_protocols_wlr::foreign_toplevel::v1::server::zwlr_foreign_toplevel_handle_v1::{
    self, ZwlrForeignToplevelHandleV1,
};
use wayland_protocols_wlr::foreign_toplevel::v1::server::zwlr_foreign_toplevel_manager_v1::{
    self, ZwlrForeignToplevelManagerV1,
};
use wayland_server::backend::{ClientId, GlobalId};
use wayland_server::protocol::wl_output::{self, WlOutput};
use wayland_server::{
    Client, DataInit, Dispatch, Display, DisplayHandle, GlobalDispatch, ListeningSocket, New,
    Resource,
};

/// Upper bound for anything a test waits on; only reached when something is broken
const TIMEOUT: Duration = Duration::from_secs(10);

const EXT: &str = "ext_foreign_toplevel_list_v1";
const WLR: &str = "zwlr_foreign_toplevel_manager_v1";

/// `zwlr_foreign_toplevel_handle_v1.state` values
const STATE_MINIMIZED: u32 = 1;
const STATE_ACTIVATED: u32 = 2;

// --- Mock Compositor Infrastructure ---

#[derive(Clone, Copy, Debug)]
enum ProtocolMode {
    Wlr,
    Ext,
    Both,
}

impl ProtocolMode {
    /// Interface the client is expected to bind in this mode
    fn expected_interface(self) -> &'static str {
        match self {
            Self::Wlr => WLR,
            Self::Ext | Self::Both => EXT,
        }
    }
}

enum ServerCommand {
    /// Announce a window; `done: false` withholds the initial `done`. wlr: enters
    /// `outputs` before that `done`.
    CreateWindow {
        id: u32,
        title: String,
        app_id: String,
        outputs: Vec<String>,
        done: bool,
    },
    /// wlr: `output_leave` for `leave`, `output_enter` for `enter`, then `state` (if
    /// given), then a single `done`; ext: bare `done`
    Place {
        id: u32,
        enter: Vec<String>,
        leave: Vec<String>,
        state: Option<Vec<u32>>,
    },
    /// Advertise a new `wl_output` global (monitor plugged in)
    AddOutput {
        name: String,
        version: u32,
    },
    /// Remove a `wl_output` global (monitor unplugged), without any `output_leave`
    RemoveOutput {
        name: String,
    },
    /// Apply all `(title, app_id)` steps, then send a single `done`
    UpdateWindow {
        id: u32,
        steps: Vec<(Option<String>, Option<String>)>,
    },
    /// wlr: send `state` (activated) + `done`; ext: bare `done`
    StateOnlyUpdate {
        id: u32,
    },
    /// For each round and window: new title + `done` (last round's title is `final-<id>`)
    Burst {
        ids: Vec<u32>,
        rounds: u32,
    },
    CloseWindow {
        id: u32,
    },
    /// Send `finished` on every bound manager/list
    Finish,
    Stop,
}

/// Things the mock observed, reported back to the test
#[derive(Debug, Clone, PartialEq, Eq)]
enum Notice {
    Bound(&'static str),
    HandleCreated {
        window: u32,
        protocol_id: u32,
    },
    HandleDestroyed {
        window: u32,
        protocol_id: u32,
    },
    /// A client bound the `wl_output` global with this name
    OutputBound(String),
    /// A client released its `wl_output` object for this name
    OutputReleased(String),
}

/// A `wl_output` global of the mock: connector name and advertised version
#[derive(Clone)]
struct OutputSpec {
    name: String,
    version: u32,
}

fn outputs(specs: &[(&str, u32)]) -> Vec<OutputSpec> {
    specs
        .iter()
        .map(|(name, version)| OutputSpec {
            name: (*name).to_string(),
            version: *version,
        })
        .collect()
}

struct MockCompositor {
    socket_path: std::path::PathBuf,
    cmd_tx: std_mpsc::Sender<ServerCommand>,
    notice_rx: std_mpsc::Receiver<Notice>,
    /// Notices already received but not yet consumed by a `wait_notice`
    seen: Vec<Notice>,
    server_thread: Option<thread::JoinHandle<()>>,
    /// Outputs advertised from the start (the client binds them with `wlr`)
    initial_outputs: Vec<String>,
    _temp_dir: tempfile::TempDir,
}

impl MockCompositor {
    fn new(mode: ProtocolMode) -> Self {
        Self::with_outputs(mode, Vec::new())
    }

    fn with_outputs(mode: ProtocolMode, initial: Vec<OutputSpec>) -> Self {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let socket_path = temp_dir.path().join("wayland-test-0");

        let (cmd_tx, cmd_rx) = std_mpsc::channel();
        let (notice_tx, notice_rx) = std_mpsc::channel();
        let (ready_tx, ready_rx) = std_mpsc::channel();
        let socket_path_clone = socket_path.clone();
        let initial_outputs = initial.iter().map(|o| o.name.clone()).collect();

        let server_thread = thread::spawn(move || {
            run_server(
                mode,
                &initial,
                &socket_path_clone,
                &cmd_rx,
                notice_tx,
                &ready_tx,
            );
        });
        ready_rx
            .recv_timeout(TIMEOUT)
            .expect("mock server did not start listening");

        Self {
            socket_path,
            cmd_tx,
            notice_rx,
            seen: Vec::new(),
            server_thread: Some(server_thread),
            initial_outputs,
            _temp_dir: temp_dir,
        }
    }

    /// Open a client connection to this mock compositor's socket.
    ///
    /// Connecting by explicit path keeps tests from touching `WAYLAND_DISPLAY` /
    /// `XDG_RUNTIME_DIR` (mutating process env from tests is unsound).
    fn connect(&self) -> wayland_client::Connection {
        let stream = std::os::unix::net::UnixStream::connect(&self.socket_path)
            .expect("Failed to connect to mock compositor socket");
        wayland_client::Connection::from_socket(stream)
            .expect("Failed to create Wayland connection to mock compositor")
    }

    /// Spawn the `pwsw` compositor thread on this mock and wait until it has bound
    fn spawn_client(&mut self, mode: ProtocolMode) -> Receiver<WindowEvent> {
        let rx = spawn_compositor_thread_on(self.connect()).expect("spawn compositor thread");
        self.wait_bound(mode.expected_interface());
        rx
    }

    /// Spawn with an explicit protocol preference, waiting until `expected` is bound
    fn spawn_client_with(
        &mut self,
        preference: ProtocolPreference,
        expected: &'static str,
    ) -> (Receiver<WindowEvent>, Capabilities) {
        let spawned = spawn_compositor_thread_on_with_preference(self.connect(), preference)
            .expect("spawn compositor thread");
        self.wait_bound(expected);
        spawned
    }

    /// Wait until the client bound `interface` (and, for `wlr`, the initial outputs)
    fn wait_bound(&mut self, interface: &'static str) {
        self.wait_notice(|n| *n == Notice::Bound(interface));
        if interface == WLR {
            for name in self.initial_outputs.clone() {
                self.wait_notice(|n| *n == Notice::OutputBound(name.clone()));
            }
        }
    }

    fn send(&self, cmd: ServerCommand) {
        self.cmd_tx.send(cmd).expect("mock server gone");
    }

    fn create_window(&self, id: u32, title: &str, app_id: &str) {
        self.create_window_on(id, title, app_id, &[]);
    }

    fn create_window_on(&self, id: u32, title: &str, app_id: &str, outputs: &[&str]) {
        self.send(ServerCommand::CreateWindow {
            id,
            title: title.into(),
            app_id: app_id.into(),
            outputs: strings(outputs),
            done: true,
        });
    }

    /// Leave `leave`, enter `enter`, set `state` (if any), then one `done`
    fn place(&self, id: u32, enter: &[&str], leave: &[&str], state: Option<&[u32]>) {
        self.send(ServerCommand::Place {
            id,
            enter: strings(enter),
            leave: strings(leave),
            state: state.map(<[u32]>::to_vec),
        });
    }

    fn update_window(&self, id: u32, title: Option<&str>, app_id: Option<&str>) {
        self.send(ServerCommand::UpdateWindow {
            id,
            steps: vec![(title.map(String::from), app_id.map(String::from))],
        });
    }

    fn close_window(&self, id: u32) {
        self.send(ServerCommand::CloseWindow { id });
    }

    /// Wait for (and consume) the first notice matching `pred`
    fn wait_notice(&mut self, pred: impl Fn(&Notice) -> bool) -> Notice {
        if let Some(pos) = self.seen.iter().position(&pred) {
            return self.seen.remove(pos);
        }
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let notice = self
                .notice_rx
                .recv_timeout(left)
                .expect("timed out waiting for mock compositor notice");
            if pred(&notice) {
                return notice;
            }
            self.seen.push(notice);
        }
    }

    /// Every notice received so far (consumed or not is irrelevant here)
    fn all_pending_notices(&mut self) -> Vec<Notice> {
        self.seen.extend(self.notice_rx.try_iter());
        self.seen.clone()
    }

    fn stop(mut self) {
        let _ = self.cmd_tx.send(ServerCommand::Stop);
        if let Some(handle) = self.server_thread.take() {
            handle.join().expect("mock server panicked");
        }
    }
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

// --- Server State & Logic ---

/// A `wl_output` global of the mock and every client's object for it
struct MockOutput {
    name: String,
    global: GlobalId,
    resources: Vec<WlOutput>,
}

struct ServerState {
    notices: std_mpsc::Sender<Notice>,
    outputs: Vec<MockOutput>,
    // Track all bound manager resources (one per client bind)
    wlr_managers: Vec<ZwlrForeignToplevelManagerV1>,
    ext_lists: Vec<ExtForeignToplevelListV1>,
    // Live window handles, tagged with the mock's window id
    wlr_handles: Vec<(u32, ZwlrForeignToplevelHandleV1)>,
    ext_handles: Vec<(u32, ExtForeignToplevelHandleV1)>,
}

impl ServerState {
    /// Clients that currently hold window handles
    fn clients(&self) -> Vec<ClientId> {
        let mut ids: Vec<ClientId> = self
            .wlr_handles
            .iter()
            .filter_map(|(_, h)| h.client())
            .chain(self.ext_handles.iter().filter_map(|(_, h)| h.client()))
            .map(|c| c.id())
            .collect();
        ids.dedup();
        ids
    }

    fn notify(&self, notice: Notice) {
        let _ = self.notices.send(notice);
    }

    /// `client`'s live object for the output named `name` (none if it did not bind it)
    fn output_for(&self, name: &str, client: Option<Client>) -> Option<WlOutput> {
        let client = client?.id();
        self.outputs
            .iter()
            .filter(|o| o.name == name)
            .flat_map(|o| o.resources.iter())
            .find(|r| r.is_alive() && r.client().is_some_and(|c| c.id() == client))
            .cloned()
    }

    fn add_output(&mut self, dh: &DisplayHandle, spec: &OutputSpec) {
        let global = dh.create_global::<Self, WlOutput, String>(spec.version, spec.name.clone());
        self.outputs.push(MockOutput {
            name: spec.name.clone(),
            global,
            resources: Vec::new(),
        });
    }

    fn remove_output(&mut self, dh: &DisplayHandle, name: &str) {
        self.outputs.retain(|o| {
            if o.name == name {
                dh.remove_global::<Self>(o.global.clone());
            }
            o.name != name
        });
    }

    fn place_window(&self, id: u32, enter: &[String], leave: &[String], state: Option<&[u32]>) {
        for (_, handle) in self.wlr_handles.iter().filter(|(w, _)| *w == id) {
            for name in leave {
                if let Some(output) = self.output_for(name, handle.client()) {
                    handle.output_leave(&output);
                }
            }
            for name in enter {
                if let Some(output) = self.output_for(name, handle.client()) {
                    handle.output_enter(&output);
                }
            }
            if let Some(flags) = state {
                handle.state(flags.iter().flat_map(|f| f.to_ne_bytes()).collect());
            }
            handle.done();
        }
        for (_, handle) in self.ext_handles.iter().filter(|(w, _)| *w == id) {
            handle.done();
        }
    }

    fn create_window(
        &mut self,
        dh: &DisplayHandle,
        id: u32,
        title: &str,
        app_id: &str,
        outputs: &[String],
        done: bool,
    ) {
        for manager in &self.wlr_managers {
            let client = manager.client().expect("manager client");
            let handle = client
                .create_resource::<ZwlrForeignToplevelHandleV1, u32, Self>(
                    dh,
                    manager.version(),
                    id,
                )
                .expect("create wlr handle");
            manager.toplevel(&handle);
            handle.title(title.into());
            handle.app_id(app_id.into());
            for name in outputs {
                if let Some(output) = self.output_for(name, handle.client()) {
                    handle.output_enter(&output);
                }
            }
            if done {
                handle.done();
            }
            self.notify(Notice::HandleCreated {
                window: id,
                protocol_id: handle.id().protocol_id(),
            });
            self.wlr_handles.push((id, handle));
        }
        for list in &self.ext_lists {
            let client = list.client().expect("list client");
            let handle = client
                .create_resource::<ExtForeignToplevelHandleV1, u32, Self>(dh, list.version(), id)
                .expect("create ext handle");
            list.toplevel(&handle);
            handle.title(title.into());
            handle.app_id(app_id.into());
            if done {
                handle.done();
            }
            self.notify(Notice::HandleCreated {
                window: id,
                protocol_id: handle.id().protocol_id(),
            });
            self.ext_handles.push((id, handle));
        }
    }

    fn update_window(&self, id: u32, steps: &[(Option<String>, Option<String>)]) {
        for (_, handle) in self.wlr_handles.iter().filter(|(w, _)| *w == id) {
            for (title, app_id) in steps {
                if let Some(t) = title {
                    handle.title(t.clone());
                }
                if let Some(a) = app_id {
                    handle.app_id(a.clone());
                }
            }
            handle.done();
        }
        for (_, handle) in self.ext_handles.iter().filter(|(w, _)| *w == id) {
            for (title, app_id) in steps {
                if let Some(t) = title {
                    handle.title(t.clone());
                }
                if let Some(a) = app_id {
                    handle.app_id(a.clone());
                }
            }
            handle.done();
        }
    }

    fn state_only_update(&self, id: u32) {
        // zwlr state enum: activated = 2, sent as a native-endian u32 array
        let activated = 2u32.to_ne_bytes().to_vec();
        for (_, handle) in self.wlr_handles.iter().filter(|(w, _)| *w == id) {
            handle.state(activated.clone());
            handle.done();
        }
        for (_, handle) in self.ext_handles.iter().filter(|(w, _)| *w == id) {
            handle.done();
        }
    }

    fn close_window(&mut self, id: u32) {
        self.wlr_handles.retain(|(w, h)| {
            if *w == id {
                h.closed();
            }
            *w != id
        });
        self.ext_handles.retain(|(w, h)| {
            if *w == id {
                h.closed();
            }
            *w != id
        });
    }

    fn finish(&mut self) {
        for manager in self.wlr_managers.drain(..) {
            manager.finished();
        }
        for list in self.ext_lists.drain(..) {
            list.finished();
        }
    }
}

/// Flush `clients`, waiting while their sockets are full; `false` if one never drains
fn flush_paced(display: &mut Display<ServerState>, clients: &[ClientId]) -> bool {
    let deadline = Instant::now() + TIMEOUT;
    for client in clients {
        loop {
            match display.backend().flush(Some(client.clone())) {
                Ok(()) => break,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(1));
                }
                Err(_) => return false,
            }
        }
    }
    true
}

fn run_server(
    mode: ProtocolMode,
    initial_outputs: &[OutputSpec],
    socket_path: &std::path::Path,
    cmd_rx: &std_mpsc::Receiver<ServerCommand>,
    notices: std_mpsc::Sender<Notice>,
    ready: &std_mpsc::Sender<()>,
) {
    let mut display = Display::<ServerState>::new().expect("display");
    let mut dh = display.handle();

    let mut state = ServerState {
        notices,
        outputs: Vec::new(),
        wlr_managers: Vec::new(),
        ext_lists: Vec::new(),
        wlr_handles: Vec::new(),
        ext_handles: Vec::new(),
    };

    if matches!(mode, ProtocolMode::Wlr | ProtocolMode::Both) {
        dh.create_global::<ServerState, ZwlrForeignToplevelManagerV1, _>(3, ());
    }
    if matches!(mode, ProtocolMode::Ext | ProtocolMode::Both) {
        dh.create_global::<ServerState, ExtForeignToplevelListV1, _>(1, ());
    }
    for spec in initial_outputs {
        state.add_output(&dh, spec);
    }

    let listener = ListeningSocket::bind(socket_path).expect("bind mock socket");
    let _ = ready.send(());

    loop {
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                ServerCommand::Stop => return,
                ServerCommand::CreateWindow {
                    id,
                    title,
                    app_id,
                    outputs,
                    done,
                } => state.create_window(&dh, id, &title, &app_id, &outputs, done),
                ServerCommand::Place {
                    id,
                    enter,
                    leave,
                    state: flags,
                } => state.place_window(id, &enter, &leave, flags.as_deref()),
                ServerCommand::AddOutput { name, version } => {
                    state.add_output(&dh, &OutputSpec { name, version });
                }
                ServerCommand::RemoveOutput { name } => state.remove_output(&dh, &name),
                ServerCommand::UpdateWindow { id, steps } => state.update_window(id, &steps),
                ServerCommand::StateOnlyUpdate { id } => state.state_only_update(id),
                ServerCommand::Burst { ids, rounds } => {
                    for round in 1..=rounds {
                        for &id in &ids {
                            let title = if round == rounds {
                                format!("final-{id}")
                            } else {
                                format!("title-{id}-{round}")
                            };
                            state.update_window(id, &[(Some(title), None)]);
                        }
                        // Push each round into the socket before writing more: the
                        // server-side buffer is only 4 KiB and overflowing it disconnects
                        // the client. Gives up (the test then times out) if the client
                        // stops reading altogether.
                        if !flush_paced(&mut display, &state.clients()) {
                            break;
                        }
                    }
                }
                ServerCommand::CloseWindow { id } => state.close_window(id),
                ServerCommand::Finish => state.finish(),
            }
        }

        if let Some(stream) = listener.accept().expect("accept") {
            dh.insert_client(stream, std::sync::Arc::new(ClientData))
                .expect("insert client");
        }

        display
            .dispatch_clients(&mut state)
            .expect("dispatch clients");
        // A client that does not read fast enough yields `WouldBlock`; retry next round
        let _ = display.flush_clients();
        thread::sleep(Duration::from_millis(2));
    }
}

// --- Dispatch Implementations ---

impl GlobalDispatch<ZwlrForeignToplevelManagerV1, ()> for ServerState {
    fn bind(
        state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrForeignToplevelManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        state.wlr_managers.push(data_init.init(resource, ()));
        state.notify(Notice::Bound(WLR));
    }
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for ServerState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &ZwlrForeignToplevelManagerV1,
        _request: zwlr_foreign_toplevel_manager_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl Dispatch<ZwlrForeignToplevelHandleV1, u32> for ServerState {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &ZwlrForeignToplevelHandleV1,
        request: zwlr_foreign_toplevel_handle_v1::Request,
        window: &u32,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if matches!(request, zwlr_foreign_toplevel_handle_v1::Request::Destroy) {
            state.notify(Notice::HandleDestroyed {
                window: *window,
                protocol_id: resource.id().protocol_id(),
            });
        }
    }
}

impl GlobalDispatch<ExtForeignToplevelListV1, ()> for ServerState {
    fn bind(
        state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ExtForeignToplevelListV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        state.ext_lists.push(data_init.init(resource, ()));
        state.notify(Notice::Bound(EXT));
    }
}

impl Dispatch<ExtForeignToplevelListV1, ()> for ServerState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &ExtForeignToplevelListV1,
        _request: ext_foreign_toplevel_list_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl Dispatch<ExtForeignToplevelHandleV1, u32> for ServerState {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &ExtForeignToplevelHandleV1,
        request: ext_foreign_toplevel_handle_v1::Request,
        window: &u32,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if matches!(request, ext_foreign_toplevel_handle_v1::Request::Destroy) {
            state.notify(Notice::HandleDestroyed {
                window: *window,
                protocol_id: resource.id().protocol_id(),
            });
        }
    }
}

impl GlobalDispatch<WlOutput, String> for ServerState {
    fn bind(
        state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<WlOutput>,
        name: &String,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let output = data_init.init(resource, name.clone());
        if output.version() >= 4 {
            output.name(name.clone());
        }
        if output.version() >= 2 {
            output.done();
        }
        if let Some(mock) = state.outputs.iter_mut().find(|o| o.name == *name) {
            mock.resources.push(output);
        }
        state.notify(Notice::OutputBound(name.clone()));
    }
}

impl Dispatch<WlOutput, String> for ServerState {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &WlOutput,
        request: wl_output::Request,
        name: &String,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if matches!(request, wl_output::Request::Release) {
            state.notify(Notice::OutputReleased(name.clone()));
        }
    }
}

struct ClientData;
impl wayland_server::backend::ClientData for ClientData {
    fn initialized(&self, _client_id: wayland_server::backend::ClientId) {}
    fn disconnected(
        &self,
        _client_id: wayland_server::backend::ClientId,
        _reason: wayland_server::backend::DisconnectReason,
    ) {
    }
}

// --- Client-side helpers ---

/// Result of waiting on the event channel
#[derive(Debug)]
enum Recv {
    Event(WindowEvent),
    Closed,
    TimedOut,
}

fn recv_within(rx: &mut Receiver<WindowEvent>, timeout: Duration) -> Recv {
    let deadline = Instant::now() + timeout;
    loop {
        match rx.try_recv() {
            Ok(event) => return Recv::Event(event),
            Err(TryRecvError::Disconnected) => return Recv::Closed,
            Err(TryRecvError::Empty) if Instant::now() >= deadline => return Recv::TimedOut,
            Err(TryRecvError::Empty) => thread::sleep(Duration::from_millis(1)),
        }
    }
}

/// Next event, failing the test (instead of hanging) if none arrives
fn recv(rx: &mut Receiver<WindowEvent>) -> WindowEvent {
    match recv_within(rx, TIMEOUT) {
        Recv::Event(event) => event,
        other => panic!("expected a window event, got {other:?}"),
    }
}

fn expect_opened(rx: &mut Receiver<WindowEvent>, app: &str, title_expected: &str) -> u64 {
    match recv(rx) {
        WindowEvent::Opened { id, app_id, title } => {
            assert_eq!((app_id.as_str(), title.as_str()), (app, title_expected));
            id
        }
        other => panic!("expected Opened({app}), got {other:?}"),
    }
}

fn expect_changed(rx: &mut Receiver<WindowEvent>, want_id: u64, app: &str, title_expected: &str) {
    match recv(rx) {
        WindowEvent::Changed { id, app_id, title } => {
            assert_eq!(
                (id, app_id.as_str(), title.as_str()),
                (want_id, app, title_expected)
            );
        }
        other => panic!("expected Changed({want_id}), got {other:?}"),
    }
}

fn expect_closed(rx: &mut Receiver<WindowEvent>, want_id: u64) {
    match recv(rx) {
        WindowEvent::Closed { id } => assert_eq!(id, want_id),
        other => panic!("expected Closed({want_id}), got {other:?}"),
    }
}

fn expect_placement(
    rx: &mut Receiver<WindowEvent>,
    want_id: u64,
    want_outputs: &[&str],
    want_minimized: bool,
) {
    match recv(rx) {
        WindowEvent::Placement {
            id,
            outputs,
            minimized,
        } => assert_eq!(
            (id, outputs, minimized),
            (want_id, strings(want_outputs), want_minimized)
        ),
        other => panic!("expected Placement({want_id}, {want_outputs:?}), got {other:?}"),
    }
}

fn expect_channel_closed(rx: &mut Receiver<WindowEvent>) {
    match recv_within(rx, TIMEOUT) {
        Recv::Closed => {}
        other => panic!("expected the event channel to close, got {other:?}"),
    }
}

// --- Tests ---

#[rstest]
#[case::wlr(ProtocolMode::Wlr)]
#[case::ext(ProtocolMode::Ext)]
fn test_backend_lifecycle_and_handle_destroy(#[case] mode: ProtocolMode) {
    let mut mock = MockCompositor::new(mode);
    let mut rx = mock.spawn_client(mode);

    mock.create_window(101, "Window", "app");
    let id = expect_opened(&mut rx, "app", "Window");

    mock.update_window(101, Some("Updated"), None);
    expect_changed(&mut rx, id, "app", "Updated");

    mock.update_window(101, None, Some("app_updated"));
    expect_changed(&mut rx, id, "app_updated", "Updated");

    mock.close_window(101);
    expect_closed(&mut rx, id);

    // The client must destroy the handle after `closed`
    mock.wait_notice(|n| matches!(n, Notice::HandleDestroyed { window: 101, .. }));

    mock.stop();
}

#[test]
fn test_priority_selection_binds_only_ext() {
    let mut mock = MockCompositor::new(ProtocolMode::Both);
    let (mut rx, caps) = mock.spawn_client_with(ProtocolPreference::Auto, EXT);
    assert_eq!(caps.backend(), Backend::Ext);
    assert!(!caps.monitor_info());

    // The mock reports windows on every bound manager, so additionally check that the
    // window arrives exactly once and that no wlr manager was ever bound.
    mock.create_window(303, "Priority Window", "both_protocols");
    let id = expect_opened(&mut rx, "both_protocols", "Priority Window");
    mock.close_window(303);
    expect_closed(&mut rx, id);
    mock.wait_notice(|n| matches!(n, Notice::HandleDestroyed { window: 303, .. }));

    let notices = mock.all_pending_notices();
    assert!(
        !notices.contains(&Notice::Bound(WLR)),
        "wlr manager must not be bound when ext is available: {notices:?}"
    );

    mock.stop();
}

#[rstest]
#[case::wlr(ProtocolMode::Wlr)]
#[case::ext(ProtocolMode::Ext)]
fn test_concurrent_windows(#[case] mode: ProtocolMode) {
    let mut mock = MockCompositor::new(mode);
    let mut rx = mock.spawn_client(mode);

    mock.create_window(100, "Window A", "app_a");
    let id_a = expect_opened(&mut rx, "app_a", "Window A");
    mock.create_window(200, "Window B", "app_b");
    let id_b = expect_opened(&mut rx, "app_b", "Window B");
    assert_ne!(id_a, id_b, "Window IDs must be distinct");

    mock.update_window(100, Some("Window A Updated"), None);
    expect_changed(&mut rx, id_a, "app_a", "Window A Updated");
    mock.update_window(200, None, Some("app_b_updated"));
    expect_changed(&mut rx, id_b, "app_b_updated", "Window B");

    mock.close_window(100);
    expect_closed(&mut rx, id_a);
    mock.close_window(200);
    expect_closed(&mut rx, id_b);

    mock.stop();
}

#[rstest]
#[case::wlr(ProtocolMode::Wlr)]
#[case::ext(ProtocolMode::Ext)]
fn test_batched_updates_emit_one_changed(#[case] mode: ProtocolMode) {
    let mut mock = MockCompositor::new(mode);
    let mut rx = mock.spawn_client(mode);

    mock.create_window(1, "t0", "a0");
    let id = expect_opened(&mut rx, "a0", "t0");

    mock.send(ServerCommand::UpdateWindow {
        id: 1,
        steps: vec![
            (Some("t1".into()), None),
            (None, Some("a1".into())),
            (Some("t2".into()), Some("a2".into())),
            (Some("t3".into()), None),
        ],
    });
    expect_changed(&mut rx, id, "a2", "t3");

    // The next event must be the close: nothing else was emitted for the batch
    mock.close_window(1);
    expect_closed(&mut rx, id);

    mock.stop();
}

#[rstest]
#[case::wlr(ProtocolMode::Wlr)]
#[case::ext(ProtocolMode::Ext)]
fn test_state_only_done_emits_nothing(#[case] mode: ProtocolMode) {
    let mut mock = MockCompositor::new(mode);
    let mut rx = mock.spawn_client(mode);

    mock.create_window(1, "title", "app");
    let id = expect_opened(&mut rx, "app", "title");

    // wlr: activated toggle + done; ext: bare done. Neither changes app_id/title.
    mock.send(ServerCommand::StateOnlyUpdate { id: 1 });
    // Same values re-sent: still no change
    mock.update_window(1, Some("title"), Some("app"));

    mock.close_window(1);
    expect_closed(&mut rx, id);

    mock.stop();
}

/// Chosen behaviour: a window closed before its first `done` is never reported (neither
/// `Opened` nor `Closed`), but its handle is still destroyed.
#[rstest]
#[case::wlr(ProtocolMode::Wlr)]
#[case::ext(ProtocolMode::Ext)]
fn test_closed_before_first_done_is_silent(#[case] mode: ProtocolMode) {
    let mut mock = MockCompositor::new(mode);
    let mut rx = mock.spawn_client(mode);

    mock.send(ServerCommand::CreateWindow {
        id: 1,
        title: "transient".into(),
        app_id: "transient_app".into(),
        outputs: Vec::new(),
        done: false,
    });
    mock.close_window(1);
    mock.wait_notice(|n| matches!(n, Notice::HandleDestroyed { window: 1, .. }));

    // The very next event is for the following window
    mock.create_window(2, "next", "next_app");
    expect_opened(&mut rx, "next_app", "next");

    mock.stop();
}

#[rstest]
#[case::wlr(ProtocolMode::Wlr)]
#[case::ext(ProtocolMode::Ext)]
fn test_finished_closes_channel(#[case] mode: ProtocolMode) {
    let mut mock = MockCompositor::new(mode);
    let mut rx = mock.spawn_client(mode);

    mock.create_window(1, "title", "app");
    expect_opened(&mut rx, "app", "title");

    mock.send(ServerCommand::Finish);
    expect_channel_closed(&mut rx);

    mock.stop();
}

#[rstest]
#[case::wlr(ProtocolMode::Wlr)]
#[case::ext(ProtocolMode::Ext)]
fn test_compositor_disconnect_closes_channel(#[case] mode: ProtocolMode) {
    let mut mock = MockCompositor::new(mode);
    let mut rx = mock.spawn_client(mode);

    mock.create_window(1, "title", "app");
    expect_opened(&mut rx, "app", "title");

    // Stopping the mock drops the display, which disconnects the client
    mock.stop();
    expect_channel_closed(&mut rx);
}

#[rstest]
#[case::wlr(ProtocolMode::Wlr)]
#[case::ext(ProtocolMode::Ext)]
fn test_ids_unique_when_protocol_ids_are_reused(#[case] mode: ProtocolMode) {
    let mut mock = MockCompositor::new(mode);
    let mut rx = mock.spawn_client(mode);

    let mut seen_ids = Vec::new();
    let mut protocol_ids = Vec::new();
    for window in 1..=3 {
        mock.create_window(window, "title", "app");
        let created = mock
            .wait_notice(|n| matches!(n, Notice::HandleCreated { window: w, .. } if *w == window));
        let id = expect_opened(&mut rx, "app", "title");
        mock.close_window(window);
        expect_closed(&mut rx, id);
        // Wait until the mock processed the destroy, so the slot is free for reuse
        mock.wait_notice(
            |n| matches!(n, Notice::HandleDestroyed { window: w, .. } if *w == window),
        );

        let Notice::HandleCreated { protocol_id, .. } = created else {
            unreachable!()
        };
        protocol_ids.push(protocol_id);
        seen_ids.push(id);
    }

    assert!(
        protocol_ids.windows(2).all(|w| w[0] == w[1]),
        "mock should recycle the protocol object ID, got {protocol_ids:?}"
    );
    let unique: std::collections::HashSet<u64> = seen_ids.iter().copied().collect();
    assert_eq!(
        unique.len(),
        seen_ids.len(),
        "window IDs must never repeat: {seen_ids:?}"
    );

    mock.stop();
}

/// A burst of updates while the consumer is not reading must not stall the Wayland
/// thread, and the consumer must still end up with each window's final state.
#[rstest]
#[case::wlr(ProtocolMode::Wlr)]
#[case::ext(ProtocolMode::Ext)]
fn test_burst_with_slow_consumer(#[case] mode: ProtocolMode) {
    const ROUNDS: u32 = 1000;
    let windows = [1u32, 2, 3];

    let mut mock = MockCompositor::new(mode);
    let mut rx = mock.spawn_client(mode);

    for &w in &windows {
        mock.create_window(w, "start", &format!("app-{w}"));
    }
    mock.send(ServerCommand::Burst {
        ids: windows.to_vec(),
        rounds: ROUNDS,
    });
    mock.close_window(3);

    // Nothing has been read yet. If the Wayland thread were blocked on the channel it
    // could never get to window 3's `closed` and destroy the handle.
    mock.wait_notice(|n| matches!(n, Notice::HandleDestroyed { window: 3, .. }));

    // Now consume slowly, checking ordering invariants on the way
    let mut model: HashMap<u64, (String, String)> = HashMap::new();
    let mut closed: Vec<u64> = Vec::new();
    let mut received = 0usize;
    let mut apply = |event: WindowEvent, model: &mut HashMap<u64, (String, String)>| {
        received += 1;
        match event {
            WindowEvent::Opened { id, app_id, title } => {
                assert!(!closed.contains(&id), "Opened after Closed for {id}");
                assert!(
                    model.insert(id, (app_id, title)).is_none(),
                    "duplicate Opened for {id}"
                );
            }
            WindowEvent::Changed { id, app_id, title } => {
                let entry = model
                    .get_mut(&id)
                    .unwrap_or_else(|| panic!("Changed for unknown/closed window {id}"));
                *entry = (app_id, title);
            }
            WindowEvent::Placement { id, .. } => {
                panic!("no window was placed on an output, got Placement for {id}");
            }
            WindowEvent::Closed { id } => {
                assert!(
                    model.remove(&id).is_some(),
                    "Closed for unknown window {id}"
                );
                closed.push(id);
            }
        }
    };

    let expected: HashMap<String, String> = [1u32, 2]
        .iter()
        .map(|w| (format!("app-{w}"), format!("final-{w}")))
        .collect();
    let snapshot = |model: &HashMap<u64, (String, String)>| -> HashMap<String, String> {
        model.values().cloned().collect()
    };

    let deadline = Instant::now() + TIMEOUT;
    while snapshot(&model) != expected {
        assert!(
            Instant::now() < deadline,
            "consumer never reached the final state; have {:?}",
            snapshot(&model)
        );
        apply(recv(&mut rx), &mut model);
        thread::sleep(Duration::from_millis(1));
    }

    // Sync point: anything still queued must be consistent with the final state
    let ids: Vec<u64> = model.keys().copied().collect();
    mock.close_window(1);
    mock.close_window(2);
    while !model.is_empty() {
        apply(recv(&mut rx), &mut model);
    }
    for id in ids {
        assert!(!model.contains_key(&id));
    }

    let total_updates = windows.len() * usize::try_from(ROUNDS).expect("fits");
    assert!(
        received < total_updates / 10,
        "expected coalescing, but the consumer received {received} events"
    );

    mock.stop();
}

// --- Placement (monitor) tests ---

/// Window opens on one monitor, moves to another (leave + enter before one `done`),
/// spans both, is minimized and restored; a state-only change emits nothing
#[test]
fn test_wlr_placement_follows_outputs_and_minimized() {
    let mut mock =
        MockCompositor::with_outputs(ProtocolMode::Wlr, outputs(&[("HDMI-A-1", 4), ("DP-1", 4)]));
    let mut rx = mock.spawn_client(ProtocolMode::Wlr);

    mock.create_window_on(1, "title", "app", &["HDMI-A-1"]);
    let id = expect_opened(&mut rx, "app", "title");
    expect_placement(&mut rx, id, &["HDMI-A-1"], false);

    // Moved: one Placement for the whole batch, nothing for the intermediate state
    mock.place(1, &["DP-1"], &["HDMI-A-1"], None);
    expect_placement(&mut rx, id, &["DP-1"], false);

    // Spans both: names sorted
    mock.place(1, &["HDMI-A-1"], &[], None);
    expect_placement(&mut rx, id, &["DP-1", "HDMI-A-1"], false);

    mock.place(1, &[], &[], Some(&[STATE_MINIMIZED]));
    expect_placement(&mut rx, id, &["DP-1", "HDMI-A-1"], true);
    mock.place(1, &[], &[], Some(&[STATE_ACTIVATED]));
    expect_placement(&mut rx, id, &["DP-1", "HDMI-A-1"], false);

    // Same placement again (activated, re-entering an output it is on): no event; the
    // next event is the close
    mock.send(ServerCommand::StateOnlyUpdate { id: 1 });
    mock.place(1, &["DP-1"], &[], Some(&[STATE_ACTIVATED]));
    mock.close_window(1);
    expect_closed(&mut rx, id);

    mock.stop();
}

/// A window that opens minimized on no output still reports its placement once; one on
/// no output and not minimized reports none
#[test]
fn test_wlr_placement_after_opened_only_when_placed() {
    let mut mock = MockCompositor::with_outputs(ProtocolMode::Wlr, outputs(&[("DP-1", 4)]));
    let mut rx = mock.spawn_client(ProtocolMode::Wlr);

    mock.create_window(1, "nowhere", "app");
    let nowhere = expect_opened(&mut rx, "app", "nowhere");
    mock.create_window(2, "hidden", "app");
    let hidden = expect_opened(&mut rx, "app", "hidden");
    mock.place(2, &[], &[], Some(&[STATE_MINIMIZED]));
    expect_placement(&mut rx, hidden, &[], true);

    mock.close_window(1);
    expect_closed(&mut rx, nowhere);
    mock.close_window(2);
    expect_closed(&mut rx, hidden);

    mock.stop();
}

/// Monitor unplugged (`global_remove` without `output_leave`): windows on it get a
/// `Placement` without it, and the client releases its `wl_output`. A monitor plugged in
/// later is bound and reported by name.
#[test]
fn test_wlr_output_hotplug() {
    let mut mock =
        MockCompositor::with_outputs(ProtocolMode::Wlr, outputs(&[("HDMI-A-1", 4), ("DP-1", 4)]));
    let mut rx = mock.spawn_client(ProtocolMode::Wlr);

    mock.create_window_on(1, "both", "app", &["HDMI-A-1", "DP-1"]);
    let id = expect_opened(&mut rx, "app", "both");
    expect_placement(&mut rx, id, &["DP-1", "HDMI-A-1"], false);
    mock.create_window_on(2, "elsewhere", "app", &["HDMI-A-1"]);
    let other = expect_opened(&mut rx, "app", "elsewhere");
    expect_placement(&mut rx, other, &["HDMI-A-1"], false);

    mock.send(ServerCommand::RemoveOutput {
        name: "DP-1".into(),
    });
    // Only the window that was on it changes
    expect_placement(&mut rx, id, &["HDMI-A-1"], false);
    mock.wait_notice(|n| *n == Notice::OutputReleased("DP-1".into()));

    mock.send(ServerCommand::AddOutput {
        name: "DP-2".into(),
        version: 4,
    });
    mock.wait_notice(|n| *n == Notice::OutputBound("DP-2".into()));
    mock.place(1, &["DP-2"], &[], None);
    expect_placement(&mut rx, id, &["DP-2", "HDMI-A-1"], false);

    mock.close_window(2);
    expect_closed(&mut rx, other);

    mock.stop();
}

/// `wl_output` before version 4 has no `name` event: a stable fallback label is used
#[test]
fn test_wlr_output_without_name_gets_fallback_label() {
    let mut mock = MockCompositor::with_outputs(ProtocolMode::Wlr, outputs(&[("unnamed", 3)]));
    let mut rx = mock.spawn_client(ProtocolMode::Wlr);

    mock.create_window_on(1, "title", "app", &["unnamed"]);
    let id = expect_opened(&mut rx, "app", "title");
    let first = match recv(&mut rx) {
        WindowEvent::Placement {
            id: got,
            outputs,
            minimized: false,
        } if got == id && outputs.len() == 1 => outputs[0].clone(),
        other => panic!("expected one fallback-labelled output, got {other:?}"),
    };
    let global = first
        .strip_prefix("output-")
        .unwrap_or_else(|| panic!("unexpected label {first:?}"));
    assert!(global.parse::<u32>().is_ok(), "label {first:?}");

    // The label is stable: leaving and re-entering reports the same one
    mock.place(1, &[], &["unnamed"], None);
    expect_placement(&mut rx, id, &[], false);
    mock.place(1, &["unnamed"], &[], None);
    expect_placement(&mut rx, id, &[first.as_str()], false);

    mock.stop();
}

/// `ext` has no placement information: no `wl_output` is bound and no `Placement` sent
#[test]
fn test_ext_backend_reports_no_placement() {
    let mut mock = MockCompositor::with_outputs(ProtocolMode::Ext, outputs(&[("HDMI-A-1", 4)]));
    let (mut rx, caps) = mock.spawn_client_with(ProtocolPreference::PreferWlr, EXT);
    assert_eq!(caps.backend(), Backend::Ext);
    assert!(!caps.monitor_info());

    mock.create_window_on(1, "title", "app", &["HDMI-A-1"]);
    let id = expect_opened(&mut rx, "app", "title");
    mock.place(1, &["HDMI-A-1"], &[], Some(&[STATE_MINIMIZED]));
    mock.close_window(1);
    expect_closed(&mut rx, id);
    mock.wait_notice(|n| matches!(n, Notice::HandleDestroyed { window: 1, .. }));

    let notices = mock.all_pending_notices();
    assert!(
        !notices.iter().any(|n| matches!(n, Notice::OutputBound(_))),
        "ext backend must not bind outputs: {notices:?}"
    );

    mock.stop();
}

/// `PreferWlr` binds wlr when both are advertised (and gets placement); `Auto` keeps ext
#[test]
fn test_prefer_wlr_picks_wlr_when_both_advertised() {
    let mut mock = MockCompositor::with_outputs(ProtocolMode::Both, outputs(&[("DP-1", 4)]));
    let (mut rx, caps) = mock.spawn_client_with(ProtocolPreference::PreferWlr, WLR);
    assert_eq!(caps.backend(), Backend::Wlr);
    assert!(caps.monitor_info());

    // Reported exactly once (the mock reports windows on every bound manager/list)
    mock.create_window_on(1, "title", "app", &["DP-1"]);
    let id = expect_opened(&mut rx, "app", "title");
    expect_placement(&mut rx, id, &["DP-1"], false);
    mock.close_window(1);
    expect_closed(&mut rx, id);
    mock.wait_notice(|n| matches!(n, Notice::HandleDestroyed { window: 1, .. }));

    let notices = mock.all_pending_notices();
    assert!(
        !notices.contains(&Notice::Bound(EXT)),
        "ext list must not be bound with PreferWlr: {notices:?}"
    );

    mock.stop();
}
