//! The status stream (`Request::Subscribe`): one update at once, then one per change

use pretty_assertions::assert_eq;

use super::*;
use crate::ipc::{EndpointStatus, RuleWindow, StatusUpdate};

/// A client end of a `Subscribe` connection served by `handle_connection`
struct Subscriber {
    client: tokio::net::UnixStream,
    server: JoinHandle<Result<()>>,
}

impl Subscriber {
    /// Connect (through `handle_connection`, like a real client) and subscribe
    async fn connect(h: &Harness) -> Self {
        Self::connect_with(h, |_| {}).await
    }

    /// [`Subscriber::connect`], adjusting the daemon's end of the socket first
    async fn connect_with(h: &Harness, adjust: impl FnOnce(&tokio::net::UnixStream)) -> Self {
        let (mut client, server) = tokio::net::UnixStream::pair().expect("socket pair");
        adjust(&server);
        let commands = h.commands.clone();
        let server = tokio::spawn(async move {
            let info = ServerInfo {
                version: "v9.9.9+test".to_string(),
                started: std::time::Instant::now(),
                daemon_manager: DaemonManager::Systemd,
                compositor: None,
            };
            handle_connection(server, &commands, &info).await
        });
        let body = serde_json::to_vec(&Request::Subscribe).expect("serialize");
        let len = u32::try_from(body.len()).expect("small message");
        within("subscribe request to be written", async {
            client.write_all(&len.to_be_bytes()).await.expect("write");
            client.write_all(&body).await.expect("write");
        })
        .await;
        Self { client, server }
    }

    /// The next message from the daemon
    async fn next(&mut self) -> Response {
        let buf = within("a status stream message", async {
            let mut len = [0_u8; 4];
            self.client.read_exact(&mut len).await.expect("read length");
            let mut buf = vec![0_u8; usize::try_from(u32::from_be_bytes(len)).expect("fits")];
            self.client.read_exact(&mut buf).await.expect("read body");
            buf
        })
        .await;
        serde_json::from_slice(&buf).expect("response json")
    }

    /// The next message, which must be a status update
    async fn update(&mut self) -> StatusUpdate {
        match self.next().await {
            Response::StatusUpdate(update) => update,
            other => panic!("expected a status update, got {other:?}"),
        }
    }

    /// Read updates until one satisfies `cond` (the halves of a joint switch may
    /// arrive as two updates)
    async fn update_until(&mut self, cond: impl Fn(&StatusUpdate) -> bool) -> StatusUpdate {
        for _ in 0..4 {
            let update = self.update().await;
            if cond(&update) {
                return update;
            }
        }
        panic!("no matching status update");
    }

    /// Whether the daemon ended the connection (no further message, end of stream)
    async fn assert_closed(mut self) {
        let mut rest = Vec::new();
        within("the daemon to hang up", self.client.read_to_end(&mut rest))
            .await
            .expect("read to end");
        assert_eq!(
            rest,
            Vec::<u8>::new(),
            "nothing more after the last message"
        );
        within("server task", self.server)
            .await
            .expect("server task")
            .expect("handled");
    }
}

/// An applied sink or source as the stream reports it (`window`: the deciding one)
fn applied(desc: &str, node: &str, window: Option<RuleWindow>) -> EndpointStatus {
    EndpointStatus {
        desc: desc.to_string(),
        node: node.to_string(),
        fallback: false,
        window,
    }
}

/// The window [`Harness::open`] opens for `app_id`, as the stream reports it
fn window(app_id: &str) -> RuleWindow {
    RuleWindow {
        app_id: app_id.to_string(),
        title: format!("{app_id} window"),
        rule: format!("{app_id} window"),
    }
}

/// The first message is the current state; then each change, and nothing for an
/// event that changes nothing a bar shows
#[tokio::test(start_paused = true)]
async fn test_subscribe_sends_state_then_each_change() {
    let h = start(base_config(), FakeSwitcher::default(), Some("speakers"));
    let mut sub = Subscriber::connect(&h).await;
    assert_eq!(sub.update().await, StatusUpdate {
        version: "v9.9.9+test".to_string(),
        protocol: crate::ipc::PROTOCOL_VERSION,
        daemon_manager: DaemonManager::Systemd,
        paused: false,
        sink: Some(applied("Speakers", "speakers", None)),
        source: None,
        sources_configured: false,
    });

    // A window no rule matches publishes the (unchanged) status: not repeated
    h.open(1, "zsh").await;
    h.wait_windows(1).await;
    h.open(2, "firefox").await;
    h.wait_applied("headphones").await;
    let update = sub.update().await;
    assert_eq!(
        update.sink,
        Some(applied("Headphones", "headphones", Some(window("firefox"))))
    );

    h.pause().await;
    let update = sub.update().await;
    assert!(update.paused);
    assert_eq!(
        update.sink,
        Some(applied("Headphones", "headphones", Some(window("firefox"))))
    );

    // A manual choice is not attributed to the window
    h.resume().await;
    assert!(!sub.update().await.paused);
    h.switch(ManualSwitch::Set("HDMI".to_string())).await;
    assert_eq!(sub.update().await.sink, Some(applied("HDMI", "hdmi", None)));

    // Shutting down ends the stream
    h.stop().await.expect("clean shutdown");
    sub.assert_closed().await;
}

/// With sources configured the update carries the microphone and its window
#[tokio::test(start_paused = true)]
async fn test_subscribe_reports_the_source() {
    let switcher = FakeSwitcher::default();
    switcher.set_current_source("mic.desk");
    let mut config = base_config().with_sources(vec![
        make_sink("mic.desk", "Desk Mic", true),
        make_sink("mic.headset", "Headset Mic", false),
    ]);
    config
        .rules
        .push(with_sources(make_rule("^discord$", None, "headphones"), &[
            "Headset Mic",
        ]));
    let h = start(config, switcher, Some("speakers"));
    let mut sub = Subscriber::connect(&h).await;
    let update = sub.update().await;
    assert!(update.sources_configured);
    assert_eq!(update.source, Some(applied("Desk Mic", "mic.desk", None)));

    h.open(1, "discord").await;
    h.wait_source("mic.headset").await;
    let update = sub
        .update_until(|u| u.source.as_ref().is_some_and(|s| s.node == "mic.headset"))
        .await;
    assert_eq!(
        update.sink,
        Some(applied("Headphones", "headphones", Some(window("discord"))))
    );
    assert_eq!(
        update.source,
        Some(applied(
            "Headset Mic",
            "mic.headset",
            Some(window("discord"))
        ))
    );
    h.stop().await.expect("clean shutdown");
}

/// At most `MAX_STATUS_SUBSCRIBERS` follow the stream; a slot frees when one leaves
#[tokio::test(start_paused = true)]
async fn test_subscriber_cap() {
    let h = start(base_config(), FakeSwitcher::default(), Some("speakers"));
    let mut held = Vec::new();
    for _ in 0..MAX_STATUS_SUBSCRIBERS {
        let mut sub = Subscriber::connect(&h).await;
        let _ = sub.update().await;
        held.push(sub);
    }
    let mut refused = Subscriber::connect(&h).await;
    let response = refused.next().await;
    let Response::Error { message } = response else {
        panic!("expected a refusal, got {response:?}");
    };
    assert!(message.contains("Too many"), "{message}");
    refused.assert_closed().await;

    // A client that hangs up frees its slot
    let gone = held.pop().expect("a subscriber");
    drop(gone.client);
    within("server task", gone.server)
        .await
        .expect("server task")
        .expect("handled");
    let mut sub = Subscriber::connect(&h).await;
    assert_eq!(
        sub.update().await.sink,
        Some(applied("Speakers", "speakers", None))
    );
    h.stop().await.expect("clean shutdown");
}

/// A subscriber that never reads does not hold up the loop: it keeps switching, and
/// the subscriber is dropped once its socket is full and a write times out
#[tokio::test(start_paused = true)]
async fn test_slow_subscriber_is_dropped_without_blocking_the_loop() {
    let h = start(base_config(), FakeSwitcher::default(), Some("speakers"));
    // The smallest send buffer: a handful of updates fill it
    let stalled = Subscriber::connect_with(&h, |server| {
        rustix::net::sockopt::set_socket_send_buffer_size(server, 1).expect("SO_SNDBUF");
    })
    .await;
    // Never polled either: a receiver that is not read costs the loop nothing
    let (tx, rx) = oneshot::channel();
    h.command(Command::Subscribe(tx)).await;
    let _idle = within("subscription", rx)
        .await
        .expect("reply")
        .expect("a slot");

    let mut switches = 0;
    while !stalled.server.is_finished() {
        assert!(switches < 2000, "the stalled subscriber was never dropped");
        h.open(1, "firefox").await;
        h.wait_applied("headphones").await;
        h.close(1).await;
        h.wait_applied("speakers").await;
        switches += 1;
        // Let the virtual clock pass the write timeout once the socket is full
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    within("server task", stalled.server)
        .await
        .expect("server task")
        .expect("handled");
    h.stop().await.expect("clean shutdown");
}
