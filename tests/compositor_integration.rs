//! Integration tests for Wayland compositor backends using a mock server.

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use wayland_protocols::ext::foreign_toplevel_list::v1::server::{
    ext_foreign_toplevel_handle_v1, ext_foreign_toplevel_list_v1,
};
use wayland_protocols_wlr::foreign_toplevel::v1::server::{
    zwlr_foreign_toplevel_handle_v1, zwlr_foreign_toplevel_manager_v1,
};
use wayland_server::{
    Client, DataInit, Dispatch, Display, DisplayHandle, GlobalDispatch, ListeningSocket, New,
    Resource, protocol::wl_output,
};

// --- Mock Compositor Infrastructure ---

#[derive(Clone, Copy)]
enum ProtocolMode {
    Wlr,
    Ext,
    Both,
}

enum ServerCommand {
    CreateWindow {
        id: u32,
        title: String,
        app_id: String,
    },
    UpdateWindow {
        id: u32,
        title: Option<String>,
        app_id: Option<String>,
    },
    CloseWindow {
        id: u32,
    },
    Stop,
}

struct MockCompositor {
    socket_path: std::path::PathBuf,
    cmd_tx: std::sync::mpsc::Sender<ServerCommand>,
    server_thread: Option<thread::JoinHandle<()>>,
    _temp_dir: tempfile::TempDir,
}

impl MockCompositor {
    fn new(mode: ProtocolMode) -> Self {
        let temp_dir = tempfile::tempdir().unwrap();
        let socket_name = "wayland-test-0";
        let socket_path = temp_dir.path().join(socket_name);

        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
        let socket_path_clone = socket_path.clone();

        let server_thread = thread::spawn(move || {
            run_server(mode, socket_path_clone, cmd_rx);
        });

        // Wait for socket to be ready
        let start = std::time::Instant::now();
        while !socket_path.exists() {
            assert!(
                start.elapsed() <= Duration::from_secs(1),
                "Timed out waiting for mock server socket"
            );
            thread::sleep(Duration::from_millis(10));
        }

        Self {
            socket_path,
            cmd_tx,
            server_thread: Some(server_thread),
            _temp_dir: temp_dir,
        }
    }

    fn create_window(&self, id: u32, title: &str, app_id: &str) {
        self.cmd_tx
            .send(ServerCommand::CreateWindow {
                id,
                title: title.to_string(),
                app_id: app_id.to_string(),
            })
            .unwrap();
    }

    fn update_window(&self, id: u32, title: Option<&str>, app_id: Option<&str>) {
        self.cmd_tx
            .send(ServerCommand::UpdateWindow {
                id,
                title: title.map(String::from),
                app_id: app_id.map(String::from),
            })
            .unwrap();
    }

    fn close_window(&self, id: u32) {
        self.cmd_tx.send(ServerCommand::CloseWindow { id }).unwrap();
    }

    fn stop(self) {
        let _ = self.cmd_tx.send(ServerCommand::Stop);
        if let Some(handle) = self.server_thread {
            handle.join().unwrap();
        }
    }
}

// --- Server State & Logic ---

struct ServerState {
    // Track all bound manager resources (one per client bind)
    wlr_managers: Vec<zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1>,
    ext_lists: Vec<ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1>,
    // Track created window handles
    wlr_handles: Vec<(
        u32,
        zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1,
    )>,
    ext_handles: Vec<(
        u32,
        ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
    )>,
}

#[allow(clippy::needless_pass_by_value)]
fn run_server(
    mode: ProtocolMode,
    socket_path: std::path::PathBuf,
    cmd_rx: std::sync::mpsc::Receiver<ServerCommand>,
) {
    let mut display = Display::<ServerState>::new().unwrap();
    let mut handle = display.handle();

    let mut state = ServerState {
        wlr_managers: Vec::new(),
        ext_lists: Vec::new(),
        wlr_handles: Vec::new(),
        ext_handles: Vec::new(),
    };

    match mode {
        ProtocolMode::Wlr => {
            handle.create_global::<ServerState, zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1, _>(3, ());
        }
        ProtocolMode::Ext => {
            handle.create_global::<ServerState, ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1, _>(1, ());
        }
        ProtocolMode::Both => {
            handle.create_global::<ServerState, zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1, _>(3, ());
            handle.create_global::<ServerState, ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1, _>(1, ());
        }
    }

    let listener = ListeningSocket::bind(&socket_path).unwrap();

    loop {
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                ServerCommand::Stop => return,
                ServerCommand::CreateWindow { id, title, app_id } => {
                    // 1. Create WLR handles for any bound managers
                    for manager in &state.wlr_managers {
                        let client = manager.client().unwrap();
                        let resource = client
                            .create_resource::<_, _, ServerState>(&handle, 1, ())
                            .unwrap();

                        manager.toplevel(&resource);
                        resource.title(title.clone());
                        resource.app_id(app_id.clone());
                        resource.done();

                        state.wlr_handles.push((id, resource));
                    }

                    // 2. Create Ext handles for any bound lists
                    for list in &state.ext_lists {
                        let client = list.client().unwrap();
                        let resource = client
                            .create_resource::<_, _, ServerState>(&handle, 1, ())
                            .unwrap();

                        list.toplevel(&resource);
                        resource.title(title.clone());
                        resource.app_id(app_id.clone());
                        resource.done();

                        state.ext_handles.push((id, resource));
                    }
                }
                ServerCommand::UpdateWindow { id, title, app_id } => {
                    // Update matching WLR handles
                    for (wid, handle) in &state.wlr_handles {
                        if *wid == id {
                            if let Some(t) = &title {
                                handle.title(t.clone());
                            }
                            if let Some(a) = &app_id {
                                handle.app_id(a.clone());
                            }
                            handle.done();
                        }
                    }
                    // Update matching Ext handles
                    for (wid, handle) in &state.ext_handles {
                        if *wid == id {
                            if let Some(t) = &title {
                                handle.title(t.clone());
                            }
                            if let Some(a) = &app_id {
                                handle.app_id(a.clone());
                            }
                            handle.done();
                        }
                    }
                }
                ServerCommand::CloseWindow { id } => {
                    // Remove all handles matching this ID
                    // (Use retain or loop + remove)
                    // Since remove shifts indices, we iterate backwards or loop until none found
                    while let Some(idx) = state.wlr_handles.iter().position(|(wid, _)| *wid == id) {
                        let (_, h) = state.wlr_handles.remove(idx);
                        h.closed();
                    }
                    while let Some(idx) = state.ext_handles.iter().position(|(wid, _)| *wid == id) {
                        let (_, h) = state.ext_handles.remove(idx);
                        h.closed();
                    }
                }
            }
        }

        if let Some(stream) = listener.accept().unwrap() {
            handle.insert_client(stream, Arc::new(ClientData)).unwrap();
        }

        display.dispatch_clients(&mut state).unwrap();
        display.flush_clients().unwrap();
        thread::sleep(Duration::from_millis(10));
    }
}

// --- Dispatch Implementations ---

impl GlobalDispatch<wl_output::WlOutput, ()> for ServerState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        _resource: New<wl_output::WlOutput>,
        _global_data: &(),
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl GlobalDispatch<zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1, ()>
    for ServerState
{
    fn bind(
        state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        state.wlr_managers.push(data_init.init(resource, ()));
    }
}

impl Dispatch<zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1, ()> for ServerState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1,
        _request: zwlr_foreign_toplevel_manager_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl Dispatch<zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1, ()> for ServerState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1,
        _request: zwlr_foreign_toplevel_handle_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl GlobalDispatch<ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1, ()> for ServerState {
    fn bind(
        state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        state.ext_lists.push(data_init.init(resource, ()));
    }
}

impl Dispatch<ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1, ()> for ServerState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1,
        request: ext_foreign_toplevel_list_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if matches!(request, ext_foreign_toplevel_list_v1::Request::Stop) {
            // Client asked to stop
        }
    }
}

impl Dispatch<ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1, ()> for ServerState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
        _request: ext_foreign_toplevel_handle_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
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

// --- Tests ---

static TEST_MUTEX: Mutex<()> = Mutex::new(());

#[test]
fn test_wlr_backend_lifecycle() {
    let _guard = TEST_MUTEX.lock().unwrap();
    let mock = MockCompositor::new(ProtocolMode::Wlr);

    let socket_name = mock.socket_path.file_name().unwrap();
    let runtime_dir = mock.socket_path.parent().unwrap();
    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", socket_name);
        std::env::set_var("XDG_RUNTIME_DIR", runtime_dir);
    }

    let mut event_rx =
        pwsw::compositor::spawn_compositor_thread().expect("Failed to spawn compositor thread");

    thread::sleep(Duration::from_millis(100)); // Wait for bind

    // 1. Open
    mock.create_window(101, "WLR Window", "wlr_app");

    let event = event_rx.blocking_recv().expect("Stream closed");
    let window_id = match event {
        pwsw::compositor::WindowEvent::Opened { id, app_id, title } => {
            assert_eq!(app_id, "wlr_app");
            assert_eq!(title, "WLR Window");
            id
        }
        _ => panic!("Expected Opened event, got {event:?}"),
    };

    // 2. Update
    mock.update_window(101, Some("WLR Updated"), None);
    let event = event_rx.blocking_recv().expect("Stream closed");
    match event {
        pwsw::compositor::WindowEvent::Changed { id, app_id, title } => {
            assert_eq!(id, window_id);
            assert_eq!(title, "WLR Updated");
            assert_eq!(app_id, "wlr_app");
        }
        _ => panic!("Expected Changed event, got {event:?}"),
    }

    // 3. Close
    mock.close_window(101);
    let event = event_rx.blocking_recv().expect("Stream closed");
    match event {
        pwsw::compositor::WindowEvent::Closed { id } => {
            assert_eq!(id, window_id);
        }
        _ => panic!("Expected Closed event, got {event:?}"),
    }

    mock.stop();
}

#[test]
fn test_ext_backend_lifecycle() {
    let _guard = TEST_MUTEX.lock().unwrap();
    let mock = MockCompositor::new(ProtocolMode::Ext);

    let socket_name = mock.socket_path.file_name().unwrap();
    let runtime_dir = mock.socket_path.parent().unwrap();
    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", socket_name);
        std::env::set_var("XDG_RUNTIME_DIR", runtime_dir);
    }

    let mut event_rx =
        pwsw::compositor::spawn_compositor_thread().expect("Failed to spawn compositor thread");

    thread::sleep(Duration::from_millis(100));

    // 1. Open
    mock.create_window(202, "Ext Window", "ext_app");

    let event = event_rx.blocking_recv().expect("Stream closed");
    let window_id = match event {
        pwsw::compositor::WindowEvent::Opened { id, app_id, title } => {
            assert_eq!(app_id, "ext_app");
            assert_eq!(title, "Ext Window");
            id
        }
        _ => panic!("Expected Opened event, got {event:?}"),
    };

    // 2. Update
    mock.update_window(202, None, Some("ext_app_updated"));
    let event = event_rx.blocking_recv().expect("Stream closed");
    match event {
        pwsw::compositor::WindowEvent::Changed { id, app_id, title } => {
            assert_eq!(id, window_id);
            assert_eq!(app_id, "ext_app_updated");
            assert_eq!(title, "Ext Window");
        }
        _ => panic!("Expected Changed event, got {event:?}"),
    }

    // 3. Close
    mock.close_window(202);
    let event = event_rx.blocking_recv().expect("Stream closed");
    match event {
        pwsw::compositor::WindowEvent::Closed { id } => {
            assert_eq!(id, window_id);
        }
        _ => panic!("Expected Closed event, got {event:?}"),
    }

    mock.stop();
}

#[test]
fn test_priority_selection() {
    let _guard = TEST_MUTEX.lock().unwrap();
    // Advertise BOTH protocols
    let mock = MockCompositor::new(ProtocolMode::Both);

    let socket_name = mock.socket_path.file_name().unwrap();
    let runtime_dir = mock.socket_path.parent().unwrap();
    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", socket_name);
        std::env::set_var("XDG_RUNTIME_DIR", runtime_dir);
    }

    // Client should prefer Ext
    let mut event_rx =
        pwsw::compositor::spawn_compositor_thread().expect("Failed to spawn compositor thread");

    thread::sleep(Duration::from_millis(100));

    // Create window
    mock.create_window(303, "Priority Window", "both_protocols");

    let event = event_rx.blocking_recv().expect("Stream closed");
    match event {
        pwsw::compositor::WindowEvent::Opened {
            id: _,
            app_id,
            title,
        } => {
            assert_eq!(app_id, "both_protocols");
            assert_eq!(title, "Priority Window");
        }
        _ => panic!("Expected Opened event, got {event:?}"),
    }

    mock.stop();
}

#[test]
fn test_ext_concurrent_windows() {
    let _guard = TEST_MUTEX.lock().unwrap();
    let mock = MockCompositor::new(ProtocolMode::Ext);

    let socket_name = mock.socket_path.file_name().unwrap();
    let runtime_dir = mock.socket_path.parent().unwrap();
    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", socket_name);
        std::env::set_var("XDG_RUNTIME_DIR", runtime_dir);
    }

    let mut event_rx =
        pwsw::compositor::spawn_compositor_thread().expect("Failed to spawn compositor thread");

    thread::sleep(Duration::from_millis(100));

    // 1. Open Window A
    mock.create_window(100, "Window A", "app_a");
    let event = event_rx.blocking_recv().expect("Stream closed");
    let id_a = match event {
        pwsw::compositor::WindowEvent::Opened { id, app_id, .. } => {
            assert_eq!(app_id, "app_a");
            id
        }
        _ => panic!("Expected Opened A, got {event:?}"),
    };

    // 2. Open Window B
    mock.create_window(200, "Window B", "app_b");
    let event = event_rx.blocking_recv().expect("Stream closed");
    let id_b = match event {
        pwsw::compositor::WindowEvent::Opened { id, app_id, .. } => {
            assert_eq!(app_id, "app_b");
            id
        }
        _ => panic!("Expected Opened B, got {event:?}"),
    };

    assert_ne!(id_a, id_b, "Window IDs must be distinct");

    // 3. Update Window A (verify it doesn't affect B)
    mock.update_window(100, Some("Window A Updated"), None);
    let event = event_rx.blocking_recv().expect("Stream closed");
    match event {
        pwsw::compositor::WindowEvent::Changed { id, title, .. } => {
            assert_eq!(id, id_a);
            assert_eq!(title, "Window A Updated");
        }
        _ => panic!("Expected Changed A, got {event:?}"),
    }

    // 4. Update Window B
    mock.update_window(200, None, Some("app_b_updated"));
    let event = event_rx.blocking_recv().expect("Stream closed");
    match event {
        pwsw::compositor::WindowEvent::Changed { id, app_id, .. } => {
            assert_eq!(id, id_b);
            assert_eq!(app_id, "app_b_updated");
        }
        _ => panic!("Expected Changed B, got {event:?}"),
    }

    // 5. Close A
    mock.close_window(100);
    let event = event_rx.blocking_recv().expect("Stream closed");
    match event {
        pwsw::compositor::WindowEvent::Closed { id } => assert_eq!(id, id_a),
        _ => panic!("Expected Closed A, got {event:?}"),
    }

    // 6. Close B
    mock.close_window(200);
    let event = event_rx.blocking_recv().expect("Stream closed");
    match event {
        pwsw::compositor::WindowEvent::Closed { id } => assert_eq!(id, id_b),
        _ => panic!("Expected Closed B, got {event:?}"),
    }

    mock.stop();
}

#[test]
fn test_wlr_concurrent_windows() {
    let _guard = TEST_MUTEX.lock().unwrap();
    let mock = MockCompositor::new(ProtocolMode::Wlr);

    let socket_name = mock.socket_path.file_name().unwrap();
    let runtime_dir = mock.socket_path.parent().unwrap();
    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", socket_name);
        std::env::set_var("XDG_RUNTIME_DIR", runtime_dir);
    }

    let mut event_rx =
        pwsw::compositor::spawn_compositor_thread().expect("Failed to spawn compositor thread");

    thread::sleep(Duration::from_millis(100));

    // 1. Open Window A
    mock.create_window(100, "Window A", "app_a");
    let event = event_rx.blocking_recv().expect("Stream closed");
    let id_a = match event {
        pwsw::compositor::WindowEvent::Opened { id, app_id, .. } => {
            assert_eq!(app_id, "app_a");
            id
        }
        _ => panic!("Expected Opened A, got {event:?}"),
    };

    // 2. Open Window B
    mock.create_window(200, "Window B", "app_b");
    let event = event_rx.blocking_recv().expect("Stream closed");
    let id_b = match event {
        pwsw::compositor::WindowEvent::Opened { id, app_id, .. } => {
            assert_eq!(app_id, "app_b");
            id
        }
        _ => panic!("Expected Opened B, got {event:?}"),
    };

    assert_ne!(id_a, id_b, "Window IDs must be distinct");

    // 3. Update Window A
    mock.update_window(100, Some("Window A Updated"), None);
    let event = event_rx.blocking_recv().expect("Stream closed");
    match event {
        pwsw::compositor::WindowEvent::Changed { id, title, .. } => {
            assert_eq!(id, id_a);
            assert_eq!(title, "Window A Updated");
        }
        _ => panic!("Expected Changed A, got {event:?}"),
    }

    // 4. Update Window B
    mock.update_window(200, None, Some("app_b_updated"));
    let event = event_rx.blocking_recv().expect("Stream closed");
    match event {
        pwsw::compositor::WindowEvent::Changed { id, app_id, .. } => {
            assert_eq!(id, id_b);
            assert_eq!(app_id, "app_b_updated");
        }
        _ => panic!("Expected Changed B, got {event:?}"),
    }

    // 5. Close A
    mock.close_window(100);
    let event = event_rx.blocking_recv().expect("Stream closed");
    match event {
        pwsw::compositor::WindowEvent::Closed { id } => assert_eq!(id, id_a),
        _ => panic!("Expected Closed A, got {event:?}"),
    }

    // 6. Close B
    mock.close_window(200);
    let event = event_rx.blocking_recv().expect("Stream closed");
    match event {
        pwsw::compositor::WindowEvent::Closed { id } => assert_eq!(id, id_b),
        _ => panic!("Expected Closed B, got {event:?}"),
    }

    mock.stop();
}
