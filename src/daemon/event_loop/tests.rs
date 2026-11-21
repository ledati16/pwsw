//! Event loop tests with a fake switcher (no `PipeWire`, no compositor, no D-Bus)
//!
//! Most run on a paused clock: retry backoffs and the reload debounce elapse
//! instantly once the runtime is idle.
//!
//! Every wait is bounded by [`WAIT`] so a bug fails the test instead of hanging it.
//! On a paused clock that bound is virtual time, which auto-advances as soon as the
//! runtime is idle: a reply that will never come fails at once.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use pretty_assertions::assert_eq;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::task::JoinHandle;

use super::*;
use crate::compositor::detected_capabilities;
use crate::daemon::ipc_handler::{ServerInfo, handle_connection};
use crate::daemon_manager::DaemonManager;
use crate::ipc::Request;
use crate::test_utils::fixtures::{
    make_config, make_rule, make_rule_with_fallbacks, make_sink, with_output, with_sources,
};

/// Upper bound for any single wait in these tests
const WAIT: Duration = Duration::from_secs(30);

/// Real-time limit for a whole harness test; see [`Watchdog`]
const WATCHDOG: Duration = Duration::from_secs(120);

/// Aborts the test process if the test that armed it is still running after
/// [`WATCHDOG`] of real time.
///
/// On a paused clock [`within`] bounds are virtual, and tokio suspends auto-advance
/// while a `spawn_blocking` task runs: a blocking task that never finished would
/// freeze every timeout and leave the test parked forever (waking every millisecond,
/// never failing). Aborting turns such a hang into a visible failure.
struct Watchdog {
    _disarm: std::sync::mpsc::Sender<()>,
}

impl Watchdog {
    fn arm() -> Self {
        let (disarm, armed) = std::sync::mpsc::channel::<()>();
        let test = std::thread::current()
            .name()
            .unwrap_or("<unnamed test>")
            .to_string();
        std::thread::spawn(move || {
            if armed.recv_timeout(WATCHDOG) == Err(std::sync::mpsc::RecvTimeoutError::Timeout) {
                eprintln!("watchdog: {test} still running after {WATCHDOG:?}; aborting");
                std::process::abort();
            }
        });
        Self { _disarm: disarm }
    }
}

/// Await `fut`, failing the test if it takes longer than [`WAIT`]
async fn within<T>(what: &str, fut: impl Future<Output = T>) -> T {
    let Ok(value) = tokio::time::timeout(WAIT, fut).await else {
        panic!("timed out after {WAIT:?} waiting for {what}");
    };
    value
}

/// Records calls; can fail a sink N times, signal when an activation starts, and
/// hold activations until released
///
/// Also plays the audio server for manual requests: `current` is the real default
/// (set by successful activations; [`start`] seeds it with `applied` unless a test set
/// it), and `unreachable` sinks are reported as unable to activate.
///
/// For candidate lists (rule fallbacks), `absent` sinks are the ones activation finds
/// unreachable: they are skipped without a call, like the real resolver does.
#[derive(Default)]
struct FakeSwitcher {
    calls: Mutex<Vec<String>>,
    failures: Mutex<HashMap<String, usize>>,
    started: Mutex<Option<mpsc::UnboundedSender<String>>>,
    gate: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    current: Mutex<Option<String>>,
    unreachable: Mutex<Vec<String>>,
    absent: Mutex<Vec<String>>,
    /// Every `activate` request's sink half: the candidates and `upgrade_from`
    requests: Mutex<Vec<(Vec<String>, Option<usize>)>>,
    /// Every `activate` request's source half
    source_requests: Mutex<Vec<(Vec<String>, Option<usize>)>>,
    /// Source activations, by name (sink ones are in `calls`)
    source_calls: Mutex<Vec<String>>,
    /// The real default source
    current_source: Mutex<Option<String>>,
    /// For every `activate` call: whether it carried a sink half and a source half
    halves: Mutex<Vec<(bool, bool)>>,
}

impl FakeSwitcher {
    fn failing(sink: &str, times: usize) -> Self {
        let fake = Self::default();
        fake.failures
            .lock()
            .expect("lock")
            .insert(sink.to_string(), times);
        fake
    }

    /// Report `sinks` as unable to activate right now (e.g. a TV that is off)
    fn unreachable(self, sinks: &[&str]) -> Self {
        *self.unreachable.lock().expect("lock") = sinks.iter().map(|s| (*s).to_string()).collect();
        self
    }

    /// Make activation find `sinks` unreachable (or reachable again, with `&[]`)
    fn set_absent(&self, sinks: &[&str]) {
        *self.absent.lock().expect("lock") = sinks.iter().map(|s| (*s).to_string()).collect();
    }

    /// The real default, as an outside program (e.g. `pavucontrol`) would set it
    fn set_current(&self, sink: &str) {
        *self.current.lock().expect("lock") = Some(sink.to_string());
    }

    /// Hold every activation until `()` is sent (or the sender is dropped, or [`WAIT`])
    fn gated() -> (
        Self,
        std::sync::mpsc::Sender<()>,
        mpsc::UnboundedReceiver<String>,
    ) {
        let fake = Self::default();
        let (gate_tx, gate_rx) = std::sync::mpsc::channel();
        let (started_tx, started_rx) = mpsc::unbounded_channel();
        *fake.gate.lock().expect("lock") = Some(gate_rx);
        *fake.started.lock().expect("lock") = Some(started_tx);
        (fake, gate_tx, started_rx)
    }
}

impl FakeSwitcher {
    fn activate_one(&self, kind: Endpoint, name: &str) -> Result<String> {
        match kind {
            Endpoint::Sink => self.calls.lock().expect("lock").push(name.to_string()),
            Endpoint::Source => self
                .source_calls
                .lock()
                .expect("lock")
                .push(name.to_string()),
        }
        if let Some(tx) = &*self.started.lock().expect("lock") {
            let _ = tx.send(name.to_string());
        }
        if let Some(gate) = &*self.gate.lock().expect("lock") {
            // Bounded: a held activation must never keep runtime shutdown waiting
            let _ = gate.recv_timeout(WAIT);
        }
        let mut failures = self.failures.lock().expect("lock");
        if let Some(left) = failures.get_mut(name)
            && *left > 0
        {
            *left -= 1;
            eyre::bail!("fake failure for {name}");
        }
        let current = match kind {
            Endpoint::Sink => &self.current,
            Endpoint::Source => &self.current_source,
        };
        *current.lock().expect("lock") = Some(name.to_string());
        Ok(format!("node.{name}"))
    }

    /// One half: like the real resolver, `absent` candidates are skipped
    fn activate_half(
        &self,
        kind: Endpoint,
        candidates: &[String],
        upgrade_from: Option<usize>,
    ) -> Result<Option<(usize, String)>> {
        let absent = self.absent.lock().expect("lock").clone();
        let reachable = |c: &String| !absent.contains(c);
        if let Some(rank) = upgrade_from
            && !candidates[..rank.min(candidates.len())]
                .iter()
                .any(reachable)
        {
            return Ok(None);
        }
        let mut errors = Vec::new();
        for (index, candidate) in candidates.iter().enumerate() {
            if !reachable(candidate) {
                errors.push(format!("{candidate} is not present"));
                continue;
            }
            match self.activate_one(kind, candidate) {
                Ok(node) => return Ok(Some((index, node))),
                Err(e) => errors.push(format!("{e:#}")),
            }
        }
        eyre::bail!("{}", errors.join("; "))
    }

    /// Set the real default source, as an outside program would
    fn set_current_source(&self, source: &str) {
        *self.current_source.lock().expect("lock") = Some(source.to_string());
    }
}

impl SinkSwitcher for FakeSwitcher {
    fn activate(
        &self,
        sink: Option<worker::Half<'_>>,
        source: Option<worker::Half<'_>>,
    ) -> (Option<worker::HalfResult>, Option<worker::HalfResult>) {
        self.halves
            .lock()
            .expect("lock")
            .push((sink.is_some(), source.is_some()));
        if let Some((candidates, upgrade_from)) = sink {
            self.requests
                .lock()
                .expect("lock")
                .push((candidates.to_vec(), upgrade_from));
        }
        if let Some((candidates, upgrade_from)) = source {
            self.source_requests
                .lock()
                .expect("lock")
                .push((candidates.to_vec(), upgrade_from));
        }
        (
            sink.map(|(c, u)| self.activate_half(Endpoint::Sink, c, u)),
            source.map(|(c, u)| self.activate_half(Endpoint::Source, c, u)),
        )
    }

    fn probe(&self, config: &Config) -> Result<crate::manual::Probe> {
        let unreachable = self.unreachable.lock().expect("lock");
        Ok(crate::manual::Probe {
            current: self.current.lock().expect("lock").clone(),
            unreachable: config
                .sinks
                .iter()
                .filter(|s| unreachable.contains(&s.name))
                .map(|s| (s.name.clone(), format!("{} is off", s.desc)))
                .collect(),
            current_source: self.current_source.lock().expect("lock").clone(),
        })
    }
}

/// Default "speakers"; steam → hdmi, mpv → usb, firefox → headphones
fn base_config() -> Config {
    let mut config = make_config(
        vec![
            make_sink("speakers", "Speakers", true),
            make_sink("hdmi", "HDMI", false),
            make_sink("headphones", "Headphones", false),
            make_sink("usb", "USB", false),
        ],
        vec![
            make_rule("^steam$", None, "hdmi"),
            make_rule("^mpv$", None, "usb"),
            make_rule("firefox", None, "headphones"),
        ],
    );
    config.settings.default_on_startup = false;
    config
}

struct Harness {
    windows: Option<mpsc::Sender<WindowEvent>>,
    commands: mpsc::Sender<Command>,
    config_changed: mpsc::UnboundedSender<()>,
    signals: mpsc::Sender<Signal>,
    notices: std::sync::mpsc::Receiver<Notice>,
    switcher: Arc<FakeSwitcher>,
    reloads: Arc<AtomicUsize>,
    statuses: Arc<Mutex<Vec<String>>>,
    config_path: PathBuf,
    task: JoinHandle<Result<()>>,
    _dir: tempfile::TempDir,
    _watchdog: Watchdog,
}

fn start(config: Config, switcher: FakeSwitcher, applied: Option<&str>) -> Harness {
    start_with(config, switcher, applied, None)
}

/// [`start`] with the compositor connection the loop is told it runs on
fn start_with(
    config: Config,
    switcher: FakeSwitcher,
    applied: Option<&str>,
    compositor: Option<CompositorInfo>,
) -> Harness {
    let watchdog = Watchdog::arm();
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join("config.toml");
    let (windows_tx, windows_rx) = mpsc::channel(64);
    let (commands_tx, commands_rx) = mpsc::channel(64);
    let (config_tx, config_rx) = mpsc::unbounded_channel();
    let (signals_tx, signals_rx) = mpsc::channel(8);
    let (notifier, notices) = Notifier::channel();
    let reloads = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&reloads);
    let statuses = Arc::new(Mutex::new(Vec::new()));
    let status_log = Arc::clone(&statuses);
    let hooks = Hooks {
        systemd: false,
        set_log_level: Some(Box::new(move |_: &str| {
            counter.fetch_add(1, Ordering::SeqCst);
        })),
        status: Some(Box::new(move |text: &str| {
            status_log.lock().expect("lock").push(text.to_string());
        })),
        after_reload: None,
    };
    {
        let mut current = switcher.current.lock().expect("lock");
        if current.is_none() {
            *current = applied.map(str::to_string);
        }
    }
    // The fake's real default source (if a test set one) is what startup found
    let applied_source = switcher.current_source.lock().expect("lock").clone();
    let switcher = Arc::new(switcher);
    let daemon = Daemon::new(
        State::new(Arc::new(config)).expect("valid config"),
        config_path.clone(),
        Arc::clone(&switcher),
        notifier,
        hooks,
        applied.map(str::to_string),
    )
    .with_applied_source(applied_source);
    let daemon = match compositor {
        Some(compositor) => daemon.with_compositor(compositor),
        None => daemon,
    };
    let task = tokio::spawn(daemon.run(Inputs {
        windows: windows_rx,
        commands: commands_rx,
        config_changed: config_rx,
        signals: signals_rx,
    }));
    Harness {
        windows: Some(windows_tx),
        commands: commands_tx,
        config_changed: config_tx,
        signals: signals_tx,
        notices,
        switcher,
        reloads,
        statuses,
        config_path,
        task,
        _dir: dir,
        _watchdog: watchdog,
    }
}

impl Harness {
    async fn event(&self, event: WindowEvent) {
        let windows = self.windows.as_ref().expect("window channel open");
        within("window event to be queued", windows.send(event))
            .await
            .expect("loop running");
    }

    async fn open(&self, id: u64, app_id: &str) {
        self.event(WindowEvent::Opened {
            id,
            app_id: app_id.to_string(),
            title: format!("{app_id} window"),
        })
        .await;
    }

    async fn close(&self, id: u64) {
        self.event(WindowEvent::Closed { id }).await;
    }

    async fn command(&self, command: Command) {
        within("command to be queued", self.commands.send(command))
            .await
            .expect("loop running");
    }

    async fn snapshot(&self) -> Snapshot {
        let (tx, rx) = oneshot::channel();
        self.command(Command::Query(tx)).await;
        within("snapshot", rx).await.expect("snapshot")
    }

    async fn switch(&self, request: ManualSwitch) -> Response {
        let (reply, rx) = oneshot::channel();
        self.command(Command::Switch { request, reply }).await;
        within("switch reply", rx).await.expect("switch reply")
    }

    async fn signal(&self, signal: Signal) {
        within("signal to be queued", self.signals.send(signal))
            .await
            .expect("loop running");
    }

    fn calls(&self) -> Vec<String> {
        self.switcher.calls.lock().expect("lock").clone()
    }

    fn requests(&self) -> Vec<(Vec<String>, Option<usize>)> {
        self.switcher.requests.lock().expect("lock").clone()
    }

    fn source_calls(&self) -> Vec<String> {
        self.switcher.source_calls.lock().expect("lock").clone()
    }

    fn source_requests(&self) -> Vec<(Vec<String>, Option<usize>)> {
        self.switcher.source_requests.lock().expect("lock").clone()
    }

    /// For each `activate` call: (had a sink half, had a source half)
    fn halves(&self) -> Vec<(bool, bool)> {
        self.switcher.halves.lock().expect("lock").clone()
    }

    async fn wait_source(&self, source: &str) -> Snapshot {
        self.wait_for(&format!("applied source = {source}"), |s, _| {
            s.applied_source.as_deref() == Some(source)
        })
        .await
    }

    fn statuses(&self) -> Vec<String> {
        self.statuses.lock().expect("lock").clone()
    }

    fn notices(&self) -> Vec<Notice> {
        self.notices.try_iter().collect()
    }

    /// Wait for the next notification
    async fn next_notice(&self) -> Notice {
        let deadline = std::time::Instant::now() + WAIT;
        loop {
            if let Ok(notice) = self.notices.try_recv() {
                return notice;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for a notification"
            );
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }

    /// Poll snapshots (letting the paused clock advance) until `cond` holds
    async fn wait_for(&self, what: &str, cond: impl Fn(&Snapshot, &[String]) -> bool) -> Snapshot {
        let deadline = std::time::Instant::now() + WAIT;
        loop {
            let snapshot = self.snapshot().await;
            if cond(&snapshot, &self.calls()) {
                return snapshot;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for {what}: applied={:?} calls={:?}",
                snapshot.applied,
                self.calls()
            );
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }

    async fn wait_applied(&self, sink: &str) -> Snapshot {
        self.wait_for(&format!("applied = {sink}"), |s, _| {
            s.applied.as_deref() == Some(sink)
        })
        .await
    }

    /// Wait until `count` windows are open (pick a count only the last event reaches)
    async fn wait_windows(&self, count: usize) -> Snapshot {
        self.wait_for(&format!("{count} windows"), |s, _| s.windows.len() == count)
            .await
    }

    async fn stop(self) -> Result<()> {
        self.signal(Signal::Terminate).await;
        within("loop to exit", self.task).await.expect("loop task")
    }
}

// ----------------------------------------------------------------------------
// Rule-driven switching
// ----------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn test_rule_switch_and_fallback() {
    let h = start(base_config(), FakeSwitcher::default(), Some("speakers"));
    h.open(1, "firefox").await;
    h.wait_applied("headphones").await;
    h.close(1).await;
    h.wait_applied("speakers").await;
    assert_eq!(h.calls(), vec!["headphones", "speakers"]);
    h.stop().await.expect("clean shutdown");
}

#[tokio::test(start_paused = true)]
async fn test_close_of_unknown_window_switches_nothing() {
    let h = start(base_config(), FakeSwitcher::default(), Some("speakers"));
    h.close(99).await;
    h.open(1, "zsh").await;
    h.wait_windows(1).await;
    assert_eq!(h.calls(), Vec::<String>::new());
    h.stop().await.expect("clean shutdown");
}

/// `default_on_startup = false`: the sink found at startup stays until the rules'
/// answer changes; an unrelated window must not switch to the default
#[tokio::test(start_paused = true)]
async fn test_no_default_on_startup_keeps_startup_sink() {
    let h = start(base_config(), FakeSwitcher::default(), Some("hdmi"));
    h.open(1, "zsh").await;
    h.close(1).await;
    h.open(2, "zsh").await;
    let snapshot = h.wait_windows(1).await;
    assert_eq!(snapshot.desired, "speakers");
    assert_eq!(snapshot.applied.as_deref(), Some("hdmi"));
    assert_eq!(h.calls(), Vec::<String>::new());

    // A real change of the rules' answer still switches
    h.open(3, "firefox").await;
    h.wait_applied("headphones").await;
    assert_eq!(h.calls(), vec!["headphones"]);
    h.stop().await.expect("clean shutdown");
}

/// A default changed outside pwsw is not fought by events that leave the rules'
/// answer unchanged; the next change of that answer switches again
#[tokio::test(start_paused = true)]
async fn test_external_change_is_not_fought() {
    let h = start(base_config(), FakeSwitcher::default(), Some("speakers"));
    h.open(1, "firefox").await;
    h.wait_applied("headphones").await;

    h.switcher.set_current("usb"); // e.g. pavucontrol
    h.open(2, "zsh").await;
    h.event(WindowEvent::Changed {
        id: 1,
        app_id: "firefox".to_string(),
        title: "another page".to_string(),
    })
    .await;
    h.open(3, "zsh").await;
    h.wait_windows(3).await;
    assert_eq!(
        h.calls(),
        vec!["headphones"],
        "no switch back to Headphones"
    );

    h.close(1).await;
    h.wait_applied("speakers").await;
    assert_eq!(h.calls(), vec!["headphones", "speakers"]);
    h.stop().await.expect("clean shutdown");
}

#[tokio::test(start_paused = true)]
async fn test_match_by_index_on_open_keeps_higher_priority_sink() {
    let mut config = base_config();
    config.settings.match_by_index = true;
    let h = start(config, FakeSwitcher::default(), Some("speakers"));
    h.open(1, "steam").await;
    h.open(2, "firefox").await;
    h.wait_windows(2).await;
    let snapshot = h.wait_applied("hdmi").await;
    assert_eq!(snapshot.desired, "hdmi");
    assert_eq!(
        h.calls(),
        vec!["hdmi"],
        "firefox (rule 2) never wins over steam"
    );
    h.stop().await.expect("clean shutdown");
}

// Real clock: tokio's paused clock does not auto-advance while a `spawn_blocking`
// task runs, and here the fake activation is held on purpose
#[tokio::test]
async fn test_activation_is_not_awaited_inline() {
    let (fake, gate, mut started) = FakeSwitcher::gated();
    let h = start(base_config(), fake, Some("speakers"));

    h.open(1, "firefox").await;
    assert_eq!(
        within("activation start", started.recv()).await.as_deref(),
        Some("headphones")
    );

    // The activation is stuck; the loop still answers queries and takes events
    h.open(2, "zsh").await;
    let snapshot = h.wait_windows(2).await;
    assert_eq!(snapshot.applied.as_deref(), Some("speakers"));
    assert_eq!(snapshot.desired, "headphones");

    gate.send(()).expect("release");
    h.wait_applied("headphones").await;
    h.stop().await.expect("clean shutdown");
}

// Real clock: tokio's paused clock does not auto-advance while a `spawn_blocking`
// task runs, and here the fake activation is held on purpose
#[tokio::test]
async fn test_rapid_changes_coalesce_to_latest() {
    let (fake, gate, mut started) = FakeSwitcher::gated();
    let h = start(base_config(), fake, Some("speakers"));

    h.open(1, "firefox").await;
    assert_eq!(
        within("activation start", started.recv()).await.as_deref(),
        Some("headphones")
    );

    // Three desired changes while the first activation is stuck: hdmi, usb, hdmi
    h.open(2, "steam").await;
    h.open(3, "mpv").await;
    h.close(3).await;
    h.open(4, "zsh").await;
    h.open(5, "zsh").await; // marker: 4 windows only after every event above
    let snapshot = h.wait_windows(4).await;
    assert_eq!(snapshot.desired, "hdmi");

    drop(gate); // release everything
    h.wait_applied("hdmi").await;
    assert_eq!(h.calls(), vec!["headphones", "hdmi"]);
    h.stop().await.expect("clean shutdown");
}

#[tokio::test(start_paused = true)]
async fn test_failure_retries_with_backoff_then_succeeds() {
    let h = start(
        base_config(),
        FakeSwitcher::failing("headphones", 2),
        Some("speakers"),
    );
    let t0 = Instant::now();
    h.open(1, "firefox").await;
    h.wait_applied("headphones").await;
    assert_eq!(h.calls(), vec!["headphones"; 3]);
    let waited = t0.elapsed();
    assert!(
        waited >= RETRY_BACKOFF[0] + RETRY_BACKOFF[1],
        "retries follow the backoff (waited {waited:?})"
    );
    h.stop().await.expect("clean shutdown");
}

#[tokio::test(start_paused = true)]
async fn test_gives_up_after_backoff_until_next_event() {
    let h = start(
        base_config(),
        FakeSwitcher::failing("headphones", 100),
        Some("speakers"),
    );
    h.open(1, "firefox").await;
    h.wait_for("three attempts", |_, calls| calls.len() == 3)
        .await;

    tokio::time::sleep(Duration::from_secs(60)).await;
    let snapshot = h.snapshot().await;
    assert_eq!(h.calls().len(), 3, "no more attempts without a new event");
    assert_eq!(
        snapshot.applied.as_deref(),
        Some("speakers"),
        "applied unchanged"
    );

    // Any new event tries again
    h.open(2, "zsh").await;
    h.wait_for("fourth attempt", |_, calls| calls.len() == 4)
        .await;
    h.stop().await.expect("clean shutdown");
}

#[tokio::test(start_paused = true)]
async fn test_default_on_startup_failure_does_not_abort() {
    let mut config = base_config();
    config.settings.default_on_startup = true;
    let h = start(config, FakeSwitcher::failing("speakers", 1), Some("hdmi"));
    h.wait_applied("speakers").await;
    assert_eq!(h.calls(), vec!["speakers", "speakers"]);
    h.stop().await.expect("clean shutdown");
}

// ----------------------------------------------------------------------------
// Fallback sinks
// ----------------------------------------------------------------------------

/// `base_config` with firefox → [headphones, hdmi]
fn fallback_config() -> Config {
    let mut config = base_config();
    config.rules[2] = make_rule_with_fallbacks("firefox", None, &["Headphones", "HDMI"]);
    config
}

fn names(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| (*n).to_string()).collect()
}

#[tokio::test(start_paused = true)]
async fn test_unreachable_first_choice_falls_back_and_says_so() {
    let switcher = FakeSwitcher::default();
    switcher.set_absent(&["headphones"]);
    let h = start(fallback_config(), switcher, Some("speakers"));
    h.open(1, "firefox").await;
    let snapshot = h.wait_applied("hdmi").await;
    assert!(snapshot.fallback);
    assert_eq!(h.calls(), vec!["hdmi"], "the unreachable sink is not tried");
    assert_eq!(h.requests(), vec![(names(&["headphones", "hdmi"]), None)]);
    assert_eq!(
        snapshot.active_window.as_deref(),
        Some("firefox window: hdmi (fallback)")
    );
    let tracked = snapshot.windows[0].tracked.as_ref().expect("tracked");
    assert_eq!(tracked.sink_name, "headphones");
    assert_eq!(tracked.fallbacks, vec![FallbackSink {
        name: "hdmi".to_string(),
        desc: "HDMI".to_string(),
    }]);
    assert_eq!(
        h.next_notice().await.body,
        "HDMI (fallback) → firefox window"
    );
    assert_eq!(
        h.statuses().last().map(String::as_str),
        Some("Sink: HDMI (fallback)")
    );

    // Closing it goes back to the default, which is nobody's fallback
    h.close(1).await;
    let snapshot = h.wait_applied("speakers").await;
    assert!(!snapshot.fallback);
    assert_eq!(
        h.statuses().last().map(String::as_str),
        Some("Sink: Speakers")
    );
    h.stop().await.expect("clean shutdown");
}

#[tokio::test(start_paused = true)]
async fn test_failed_activation_of_first_choice_tries_the_next() {
    let h = start(
        fallback_config(),
        FakeSwitcher::failing("headphones", 1),
        Some("speakers"),
    );
    h.open(1, "firefox").await;
    let snapshot = h.wait_applied("hdmi").await;
    assert!(snapshot.fallback);
    assert_eq!(h.calls(), vec!["headphones", "hdmi"]);
    assert_eq!(h.requests().len(), 1, "one job, no retry");
    h.stop().await.expect("clean shutdown");
}

#[tokio::test(start_paused = true)]
async fn test_no_reachable_candidate_retries_then_stalls_until_next_event() {
    let switcher = FakeSwitcher::failing("hdmi", 100);
    switcher.set_absent(&["headphones"]);
    let h = start(fallback_config(), switcher, Some("speakers"));
    let t0 = Instant::now();
    h.open(1, "firefox").await;
    h.wait_for("three attempts", |_, calls| calls.len() == 3)
        .await;
    assert!(t0.elapsed() >= RETRY_BACKOFF[0] + RETRY_BACKOFF[1]);
    // Every retry asks for the whole list again
    assert_eq!(h.requests(), vec![
        (names(&["headphones", "hdmi"]), None);
        3
    ]);

    tokio::time::sleep(Duration::from_secs(60)).await;
    let snapshot = h.snapshot().await;
    assert_eq!(h.calls().len(), 3, "no more attempts without a new event");
    assert_eq!(snapshot.applied.as_deref(), Some("speakers"));
    assert!(!snapshot.fallback);

    // The next event tries again: now the first choice is back
    h.switcher.set_absent(&[]);
    h.open(2, "zsh").await;
    let snapshot = h.wait_applied("headphones").await;
    assert!(!snapshot.fallback);
    h.stop().await.expect("clean shutdown");
}

#[tokio::test(start_paused = true)]
async fn test_applied_fallback_upgrades_on_a_later_event() {
    let switcher = FakeSwitcher::default();
    switcher.set_absent(&["headphones"]);
    let h = start(fallback_config(), switcher, Some("speakers"));
    h.open(1, "firefox").await;
    h.wait_applied("hdmi").await;
    assert_eq!(
        h.next_notice().await.body,
        "HDMI (fallback) → firefox window"
    );

    // Still unreachable: an event checks, and changes nothing
    tokio::time::sleep(UPGRADE_CHECK_INTERVAL).await;
    h.open(2, "zsh").await;
    h.wait_for("an upgrade check", |_, _| h.requests().len() == 2)
        .await;
    assert_eq!(h.requests()[1], (names(&["headphones", "hdmi"]), Some(1)));
    let snapshot = h.wait_windows(2).await;
    assert_eq!(snapshot.applied.as_deref(), Some("hdmi"));
    assert!(snapshot.fallback);
    assert_eq!(h.calls(), vec!["hdmi"]);

    // Checks are rate-limited: an event right after does not check again
    h.open(3, "zsh").await;
    h.wait_windows(3).await;
    assert_eq!(h.requests().len(), 2);

    // The headphones come back: the next event after the interval upgrades
    h.switcher.set_absent(&[]);
    tokio::time::sleep(UPGRADE_CHECK_INTERVAL).await;
    h.close(3).await;
    let snapshot = h.wait_applied("headphones").await;
    assert!(!snapshot.fallback);
    assert_eq!(h.calls(), vec!["hdmi", "headphones"]);
    assert_eq!(h.next_notice().await.body, "Headphones → firefox window");
    assert_eq!(
        h.statuses().last().map(String::as_str),
        Some("Sink: Headphones")
    );

    // On the first choice: no more checks
    tokio::time::sleep(UPGRADE_CHECK_INTERVAL).await;
    h.close(2).await;
    h.wait_windows(1).await;
    assert_eq!(h.requests().len(), 3);
    h.stop().await.expect("clean shutdown");
}

#[tokio::test(start_paused = true)]
async fn test_manual_choice_is_not_upgraded_away() {
    let switcher = FakeSwitcher::default();
    switcher.set_absent(&["headphones"]);
    let h = start(fallback_config(), switcher, Some("speakers"));
    h.open(1, "firefox").await;
    h.wait_applied("hdmi").await;
    let response = h.switch(ManualSwitch::Set("USB".to_string())).await;
    assert!(
        matches!(response, Response::SinkSwitched { .. }),
        "{response:?}"
    );
    let snapshot = h.snapshot().await;
    assert!(!snapshot.fallback);

    h.switcher.set_absent(&[]);
    tokio::time::sleep(UPGRADE_CHECK_INTERVAL).await;
    h.open(2, "zsh").await;
    let snapshot = h.wait_windows(2).await;
    assert_eq!(
        snapshot.applied.as_deref(),
        Some("usb"),
        "the manual choice holds"
    );
    assert_eq!(h.requests().len(), 2, "no upgrade check");
    h.stop().await.expect("clean shutdown");
}

/// A fallback is the rules' answer too: a reload that keeps the list does not switch
#[tokio::test(start_paused = true)]
async fn test_fallback_list_is_the_desired_answer_on_reload() {
    let switcher = FakeSwitcher::default();
    switcher.set_absent(&["headphones"]);
    let h = start(fallback_config(), switcher, Some("speakers"));
    h.open(1, "firefox").await;
    h.wait_applied("hdmi").await;
    let mut config = fallback_config();
    config.settings.log_level = "debug".to_string();
    config.save_to(&h.config_path).expect("write config");
    h.signal(Signal::Hangup).await;
    h.wait_for("reload", |_, _| h.reloads.load(Ordering::SeqCst) == 1)
        .await;
    // An upgrade check may run (and find nothing better); nothing is switched
    let snapshot = h.snapshot().await;
    assert_eq!(snapshot.applied.as_deref(), Some("hdmi"));
    assert_eq!(h.calls(), vec!["hdmi"]);
    assert!(
        h.requests()
            .iter()
            .skip(1)
            .all(|(_, upgrade)| upgrade.is_some())
    );
    h.stop().await.expect("clean shutdown");
}

// ----------------------------------------------------------------------------
// Notifications
// ----------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn test_notifications_come_from_the_deciding_window() {
    let mut config = base_config();
    config.rules[0].desc = Some("Steam".to_string());
    config.rules[2].notify = Some(false); // firefox: silent
    let h = start(config, FakeSwitcher::default(), Some("speakers"));

    h.open(1, "steam").await;
    h.wait_applied("hdmi").await;
    h.open(2, "firefox").await;
    h.wait_applied("headphones").await;
    // Firefox closes: back to steam's sink, announced as steam's (not firefox's)
    h.close(2).await;
    h.wait_applied("hdmi").await;
    // Steam closes: back to the default, announced with what closed
    h.close(1).await;
    h.wait_applied("speakers").await;

    let bodies: Vec<String> = h.notices().into_iter().map(|n| n.body).collect();
    assert_eq!(bodies, vec![
        "HDMI → Steam".to_string(),
        "HDMI → Steam".to_string(),
        "Speakers → Steam closed".to_string(),
    ]);
    h.stop().await.expect("clean shutdown");
}

#[tokio::test(start_paused = true)]
async fn test_silent_rule_gets_no_closed_notification() {
    let mut config = base_config();
    config.rules[2].notify = Some(false);
    let h = start(config, FakeSwitcher::default(), Some("speakers"));
    h.open(1, "firefox").await;
    h.wait_applied("headphones").await;
    h.close(1).await;
    h.wait_applied("speakers").await;
    assert_eq!(h.notices(), vec![]);
    h.stop().await.expect("clean shutdown");
}

// ----------------------------------------------------------------------------
// Manual switching
// ----------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn test_set_sink_updates_applied_and_holds_until_rule_change() {
    let h = start(base_config(), FakeSwitcher::default(), Some("speakers"));

    let response = h.switch(ManualSwitch::Set("HDMI".to_string())).await;
    let Response::SinkSwitched { desc, node_name } = response else {
        panic!("expected SinkSwitched, got {response:?}");
    };
    assert_eq!((desc.as_str(), node_name.as_str()), ("HDMI", "node.hdmi"));
    assert_eq!(h.snapshot().await.applied.as_deref(), Some("hdmi"));

    // Unrelated window: the manual choice stays
    h.open(1, "zsh").await;
    h.wait_windows(1).await;
    assert_eq!(h.calls(), vec!["hdmi"]);

    // A rule-driven change takes over again
    h.open(2, "firefox").await;
    h.wait_applied("headphones").await;
    assert_eq!(h.calls(), vec!["hdmi", "headphones"]);
    h.stop().await.expect("clean shutdown");
}

/// The reported bug: audio on a manually chosen sink, the next manual switch fails
/// (the TV is off). Audio must stay put, and the earlier manual choice keep holding;
/// nothing else changes (no window events, the rules want the default throughout).
#[tokio::test(start_paused = true)]
async fn test_failed_manual_switch_keeps_audio_and_previous_manual_choice() {
    let h = start(
        base_config(),
        FakeSwitcher::failing("hdmi", 1),
        Some("speakers"),
    );
    let response = h.switch(ManualSwitch::Set("Headphones".to_string())).await;
    assert!(
        matches!(response, Response::SinkSwitched { .. }),
        "{response:?}"
    );
    assert!(h.snapshot().await.manual_override);

    let response = h.switch(ManualSwitch::Set("HDMI".to_string())).await;
    assert!(
        matches!(&response, Response::Error { message }
            if message.starts_with("Failed to activate sink 'HDMI'")
                && message.contains("fake failure")),
        "got {response:?}"
    );

    tokio::time::sleep(Duration::from_secs(60)).await; // any retry would have run
    let snapshot = h.snapshot().await;
    assert_eq!(snapshot.applied.as_deref(), Some("headphones"));
    assert!(snapshot.manual_override, "the earlier manual choice holds");
    assert_eq!(snapshot.desired, "speakers");
    assert_eq!(h.calls(), vec!["headphones", "hdmi"], "no move to Speakers");
    h.stop().await.expect("clean shutdown");
}

/// Audio on the rules' sink, a manual switch fails: nothing changes at all
#[tokio::test(start_paused = true)]
async fn test_failed_manual_switch_from_rule_target_changes_nothing() {
    let h = start(
        base_config(),
        FakeSwitcher::failing("hdmi", 1),
        Some("speakers"),
    );
    let response = h.switch(ManualSwitch::Set("HDMI".to_string())).await;
    assert!(matches!(response, Response::Error { .. }), "{response:?}");

    tokio::time::sleep(Duration::from_secs(60)).await;
    let snapshot = h.snapshot().await;
    assert_eq!(snapshot.applied.as_deref(), Some("speakers"));
    assert!(!snapshot.manual_override, "rules stay in charge");
    assert_eq!(h.calls(), vec!["hdmi"]);
    assert_eq!(h.notices(), vec![], "no switch notification for a failure");
    h.stop().await.expect("clean shutdown");
}

#[tokio::test(start_paused = true)]
async fn test_next_and_prev_cycle_from_real_default() {
    let h = start(base_config(), FakeSwitcher::default(), Some("speakers"));
    h.switch(ManualSwitch::Next).await;
    assert_eq!(h.snapshot().await.applied.as_deref(), Some("hdmi"));
    h.switch(ManualSwitch::Prev).await;
    h.switch(ManualSwitch::Prev).await;
    assert_eq!(h.snapshot().await.applied.as_deref(), Some("usb"));

    // Changed outside pwsw: the cycle starts from there, and pwsw learns of it
    h.switcher.set_current("headphones");
    let response = h.switch(ManualSwitch::Next).await;
    assert!(
        matches!(&response, Response::SinkSwitched { desc, .. } if desc == "USB"),
        "{response:?}"
    );
    assert_eq!(h.calls(), vec!["hdmi", "speakers", "usb", "usb"]);
    h.stop().await.expect("clean shutdown");
}

#[tokio::test(start_paused = true)]
async fn test_cycle_skips_unreachable_sinks() {
    let h = start(
        base_config(),
        FakeSwitcher::default().unreachable(&["hdmi"]),
        Some("speakers"),
    );
    let response = h.switch(ManualSwitch::Next).await;
    assert!(
        matches!(&response, Response::SinkSwitched { desc, .. } if desc == "Headphones"),
        "{response:?}"
    );
    let response = h.switch(ManualSwitch::Prev).await;
    assert!(
        matches!(&response, Response::SinkSwitched { desc, .. } if desc == "Speakers"),
        "{response:?}"
    );
    assert_eq!(
        h.calls(),
        vec!["headphones", "speakers"],
        "HDMI never tried"
    );
    h.stop().await.expect("clean shutdown");
}

#[tokio::test(start_paused = true)]
async fn test_cycle_with_nothing_reachable_reports_and_stays() {
    let h = start(
        base_config(),
        FakeSwitcher::default().unreachable(&["speakers", "hdmi", "usb"]),
        Some("headphones"),
    );
    let response = h.switch(ManualSwitch::Next).await;
    let Response::Error { message } = response else {
        panic!("expected an error, got {response:?}");
    };
    assert!(
        message.starts_with("No other sink can be activated right now:"),
        "{message}"
    );
    assert!(message.contains("HDMI: HDMI is off"), "{message}");
    let snapshot = h.snapshot().await;
    assert_eq!(snapshot.applied.as_deref(), Some("headphones"));
    assert!(!snapshot.manual_override);
    assert_eq!(h.calls(), Vec::<String>::new());
    h.stop().await.expect("clean shutdown");
}

/// An explicit `set-sink` still tries an unreachable sink, and reports its error
#[tokio::test(start_paused = true)]
async fn test_set_sink_still_tries_unreachable_sink() {
    let h = start(
        base_config(),
        FakeSwitcher::failing("hdmi", 1).unreachable(&["hdmi"]),
        Some("speakers"),
    );
    let response = h.switch(ManualSwitch::Set("HDMI".to_string())).await;
    assert!(
        matches!(&response, Response::Error { message } if message.contains("fake failure")),
        "{response:?}"
    );
    assert_eq!(h.calls(), vec!["hdmi"]);
    h.stop().await.expect("clean shutdown");
}

/// The smart toggle compares with the real default, not pwsw's last activation
#[tokio::test(start_paused = true)]
async fn test_smart_toggle_uses_real_default() {
    let h = start(base_config(), FakeSwitcher::default(), Some("speakers"));
    h.switcher.set_current("hdmi"); // changed outside pwsw
    let response = h.switch(ManualSwitch::Set("HDMI".to_string())).await;
    assert!(
        matches!(&response, Response::SinkSwitched { desc, .. } if desc == "Speakers"),
        "toggles back to the default: {response:?}"
    );
    assert_eq!(h.calls(), vec!["speakers"]);

    let response = h.switch(ManualSwitch::Set("Speakers".to_string())).await;
    assert!(
        matches!(&response, Response::Ok { message } if message == "Already on: Speakers"),
        "{response:?}"
    );
    let response = h.switch(ManualSwitch::Set("nope".to_string())).await;
    assert!(
        matches!(&response, Response::Error { message } if message.starts_with("Unknown sink")),
        "{response:?}"
    );
    assert_eq!(h.calls(), vec!["speakers"]);
    h.stop().await.expect("clean shutdown");
}

/// Queue a manual request without waiting for its reply; returns once the loop has
/// handed it to the worker (a query behind it on the same channel was answered)
async fn queue_switch(h: &Harness, request: ManualSwitch) -> oneshot::Receiver<Response> {
    let (reply, rx) = oneshot::channel();
    h.command(Command::Switch { request, reply }).await;
    h.snapshot().await;
    rx
}

/// A rule-driven switch queued behind a running activation is replaced by a manual
/// request that then fails: the rules' sink must still be activated afterwards
// Real clock: activations are held on purpose (see `test_activation_is_not_awaited_inline`)
#[tokio::test]
async fn test_failed_manual_switch_resumes_the_rule_switch_it_replaced() {
    let (fake, gate, mut started) = FakeSwitcher::gated();
    fake.failures
        .lock()
        .expect("lock")
        .insert("usb".to_string(), 1);
    let h = start(base_config(), fake, Some("speakers"));

    h.open(1, "steam").await;
    assert_eq!(
        within("activation start", started.recv()).await.as_deref(),
        Some("hdmi")
    );
    h.open(2, "firefox").await; // headphones: queued behind the held hdmi switch
    assert_eq!(h.wait_windows(2).await.desired, "headphones");
    let reply = queue_switch(&h, ManualSwitch::Set("USB".to_string())).await;

    drop(gate); // release everything
    let response = within("switch reply", reply).await.expect("reply");
    assert!(
        matches!(&response, Response::Error { message } if message.contains("fake failure")),
        "{response:?}"
    );
    let snapshot = h.wait_applied("headphones").await;
    assert!(!snapshot.manual_override);
    assert_eq!(h.calls(), vec!["hdmi", "usb", "headphones"]);
    h.stop().await.expect("clean shutdown");
}

/// As above, but the older rule switch fails and the manual request finds nothing to
/// do ("Already on"): the rules' current sink must still be activated
// Real clock: activations are held on purpose
#[tokio::test]
async fn test_noop_manual_switch_resumes_the_rule_switch_it_replaced() {
    let (fake, gate, mut started) = FakeSwitcher::gated();
    fake.failures
        .lock()
        .expect("lock")
        .insert("hdmi".to_string(), 1);
    let h = start(base_config(), fake, Some("speakers"));

    h.open(1, "steam").await;
    assert_eq!(
        within("activation start", started.recv()).await.as_deref(),
        Some("hdmi")
    );
    h.open(2, "firefox").await;
    assert_eq!(h.wait_windows(2).await.desired, "headphones");
    let reply = queue_switch(&h, ManualSwitch::Set("Speakers".to_string())).await;

    drop(gate);
    let response = within("switch reply", reply).await.expect("reply");
    assert!(
        matches!(&response, Response::Ok { message } if message == "Already on: Speakers"),
        "{response:?}"
    );
    let snapshot = h.wait_applied("headphones").await;
    assert!(!snapshot.manual_override);
    assert_eq!(h.calls(), vec!["hdmi", "headphones"]);
    h.stop().await.expect("clean shutdown");
}

/// The rule switch itself is running when a manual request queues behind it; it
/// fails (not the latest job, so no retry is scheduled) and the manual request finds
/// nothing to do: the rules' sink gets another try at once
// Real clock: activations are held on purpose
#[tokio::test]
async fn test_rule_failure_behind_noop_manual_switch_is_retried() {
    let (fake, gate, mut started) = FakeSwitcher::gated();
    fake.failures
        .lock()
        .expect("lock")
        .insert("headphones".to_string(), 1);
    let h = start(base_config(), fake, Some("speakers"));

    h.open(1, "firefox").await;
    assert_eq!(
        within("activation start", started.recv()).await.as_deref(),
        Some("headphones")
    );
    let reply = queue_switch(&h, ManualSwitch::Set("Speakers".to_string())).await;

    drop(gate);
    let response = within("switch reply", reply).await.expect("reply");
    assert!(matches!(response, Response::Ok { .. }), "{response:?}");
    h.wait_applied("headphones").await;
    assert_eq!(h.calls(), vec!["headphones", "headphones"]);
    h.stop().await.expect("clean shutdown");
}

/// A manual request that replaced a rule switch and succeeds holds as usual
// Real clock: activations are held on purpose
#[tokio::test]
async fn test_successful_manual_switch_over_queued_rule_switch_holds() {
    let (fake, gate, mut started) = FakeSwitcher::gated();
    let h = start(base_config(), fake, Some("speakers"));

    h.open(1, "steam").await;
    assert_eq!(
        within("activation start", started.recv()).await.as_deref(),
        Some("hdmi")
    );
    h.open(2, "firefox").await;
    assert_eq!(h.wait_windows(2).await.desired, "headphones");
    let reply = queue_switch(&h, ManualSwitch::Set("USB".to_string())).await;

    drop(gate);
    let response = within("switch reply", reply).await.expect("reply");
    assert!(
        matches!(response, Response::SinkSwitched { .. }),
        "{response:?}"
    );
    h.open(3, "zsh").await;
    let snapshot = h.wait_windows(3).await;
    assert_eq!(snapshot.applied.as_deref(), Some("usb"));
    assert!(snapshot.manual_override);
    assert_eq!(h.calls(), vec!["hdmi", "usb"]);
    h.stop().await.expect("clean shutdown");
}

/// `set-sink` over a real `IPC` stream: request in, `SinkSwitched` out
#[tokio::test]
async fn test_set_sink_over_ipc_stream() {
    let h = start(base_config(), FakeSwitcher::default(), Some("speakers"));
    let (mut client, server) = tokio::net::UnixStream::pair().expect("socket pair");
    let commands = h.commands.clone();
    let server_task = tokio::spawn(async move {
        let info = ServerInfo {
            version: "test".to_string(),
            started: std::time::Instant::now(),
            daemon_manager: DaemonManager::Direct,
            compositor: None,
        };
        handle_connection(server, &commands, &info).await
    });

    let body = serde_json::to_vec(&Request::SetSink {
        sink: "Headphones".to_string(),
    })
    .expect("serialize");
    let len = u32::try_from(body.len()).expect("small message");
    within("request to be written", async {
        client.write_all(&len.to_be_bytes()).await.expect("write");
        client.write_all(&body).await.expect("write");
    })
    .await;

    let buf = within("response", async {
        let mut len = [0_u8; 4];
        client.read_exact(&mut len).await.expect("read length");
        let mut buf = vec![0_u8; usize::try_from(u32::from_be_bytes(len)).expect("fits")];
        client.read_exact(&mut buf).await.expect("read body");
        buf
    })
    .await;
    let response: Response = serde_json::from_slice(&buf).expect("response json");

    assert!(
        matches!(&response, Response::SinkSwitched { node_name, .. } if node_name == "node.headphones"),
        "got {response:?}"
    );
    within("server task", server_task)
        .await
        .expect("server task")
        .expect("handled");
    assert_eq!(h.snapshot().await.applied.as_deref(), Some("headphones"));
    h.stop().await.expect("clean shutdown");
}

/// Send one raw `IPC` frame to `handle_connection` and return the parsed reply
async fn raw_ipc_exchange(h: &Harness, body: &[u8]) -> Response {
    raw_ipc_exchange_with(h, body, None).await
}

/// [`raw_ipc_exchange`] with a server that runs on `compositor`
async fn raw_ipc_exchange_with(
    h: &Harness,
    body: &[u8],
    compositor: Option<Capabilities>,
) -> Response {
    let (mut client, server) = tokio::net::UnixStream::pair().expect("socket pair");
    let commands = h.commands.clone();
    let server_task = tokio::spawn(async move {
        let info = ServerInfo {
            version: "v9.9.9+test".to_string(),
            started: std::time::Instant::now(),
            daemon_manager: DaemonManager::Direct,
            compositor,
        };
        handle_connection(server, &commands, &info).await
    });

    let len = u32::try_from(body.len()).expect("small message");
    within("request to be written", async {
        client.write_all(&len.to_be_bytes()).await.expect("write");
        client.write_all(body).await.expect("write");
    })
    .await;

    let buf = within("response", async {
        let mut len = [0_u8; 4];
        client.read_exact(&mut len).await.expect("read length");
        let mut buf = vec![0_u8; usize::try_from(u32::from_be_bytes(len)).expect("fits")];
        client.read_exact(&mut buf).await.expect("read body");
        buf
    })
    .await;
    within("server task", server_task)
        .await
        .expect("server task")
        .expect("handled");
    serde_json::from_slice(&buf).expect("response json")
}

/// A request this daemon cannot parse (e.g. from a newer client) gets an error
/// naming the daemon's version and protocol, not a dropped connection
#[tokio::test]
async fn test_unparseable_request_gets_versioned_error() {
    let h = start(base_config(), FakeSwitcher::default(), Some("speakers"));
    for body in [
        &br#"{"type":"SomeFutureRequest","arg":1}"#[..],
        &b"not json at all"[..],
    ] {
        let response = raw_ipc_exchange(&h, body).await;
        let Response::Error { message } = response else {
            panic!("expected an error reply, got {response:?}");
        };
        assert!(message.contains("v9.9.9+test"), "{message}");
        assert!(
            message.contains(&format!("protocol {}", crate::ipc::PROTOCOL_VERSION)),
            "{message}"
        );
        assert!(
            message.contains("systemctl --user restart pwsw"),
            "{message}"
        );
    }
    // The daemon keeps serving afterwards
    assert_eq!(h.snapshot().await.applied.as_deref(), Some("speakers"));
    h.stop().await.expect("clean shutdown");
}

/// `Status` reports the daemon's version and `IPC` protocol
#[tokio::test]
async fn test_status_reports_version_and_protocol() {
    let h = start(base_config(), FakeSwitcher::default(), Some("speakers"));
    let body = serde_json::to_vec(&Request::Status).expect("serialize");
    let response = raw_ipc_exchange(&h, &body).await;
    let Response::Status {
        version, protocol, ..
    } = response
    else {
        panic!("expected a status reply, got {response:?}");
    };
    assert_eq!(version, "v9.9.9+test");
    assert_eq!(protocol, crate::ipc::PROTOCOL_VERSION);
    h.stop().await.expect("clean shutdown");
}

/// `Status` reports the sink the daemon applied, never the rules' unconfirmed wish
#[tokio::test]
async fn test_status_reports_no_sink_while_the_default_is_unknown() {
    let body = serde_json::to_vec(&Request::Status).expect("serialize");
    for (applied, expected) in [(None, None), (Some("hdmi"), Some("HDMI"))] {
        let h = start(base_config(), FakeSwitcher::default(), applied);
        let response = raw_ipc_exchange(&h, &body).await;
        let Response::Status { current_sink, .. } = response else {
            panic!("expected a status reply, got {response:?}");
        };
        assert_eq!(current_sink.as_deref(), expected, "applied = {applied:?}");
        h.stop().await.expect("clean shutdown");
    }
}

// ----------------------------------------------------------------------------
// Status line (`STATUS=`)
// ----------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn test_status_is_published_at_startup() {
    for (applied, expected) in [
        (Some("hdmi"), "Sink: HDMI"),
        (
            Some("alsa_output.other"),
            "Sink: alsa_output.other (not in config)",
        ),
        (None, "Sink: unknown"),
    ] {
        let h = start(base_config(), FakeSwitcher::default(), applied);
        let _ = h.snapshot().await; // the loop has started
        assert_eq!(h.statuses(), vec![expected.to_string()]);
        h.stop().await.expect("clean shutdown");
    }
}

#[tokio::test(start_paused = true)]
async fn test_status_follows_switches_and_reloads() {
    let h = start(base_config(), FakeSwitcher::default(), Some("speakers"));
    h.open(1, "firefox").await;
    h.wait_applied("headphones").await;

    // A reload that renames the current sink needs no switch, but a new status
    let mut config = base_config();
    config.sinks[2].desc = "Studio Headphones".to_string();
    config.rules[2].sink_ref = "Studio Headphones".to_string();
    config.save_to(&h.config_path).expect("write config");
    h.signal(Signal::Hangup).await;
    h.wait_for("reload", |s, _| {
        s.config.sinks[2].desc == "Studio Headphones"
    })
    .await;

    assert_eq!(h.statuses(), vec![
        "Sink: Speakers".to_string(),
        "Sink: Headphones".to_string(),
        "Sink: Studio Headphones".to_string(),
    ]);
    assert_eq!(h.calls(), vec!["headphones"]);
    h.stop().await.expect("clean shutdown");
}

// ----------------------------------------------------------------------------
// Reload
// ----------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn test_reload_debounce_coalesces_a_burst() {
    let h = start(base_config(), FakeSwitcher::default(), Some("speakers"));
    base_config().save_to(&h.config_path).expect("write config");

    for _ in 0..5 {
        h.config_changed.send(()).expect("loop running");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    let _ = h.snapshot().await; // the reload runs inline: this waits for it

    assert_eq!(h.reloads.load(Ordering::SeqCst), 1);
    let reloaded: Vec<String> = h
        .notices()
        .into_iter()
        .filter(|n| n.kind == NoticeKind::Daemon)
        .map(|n| n.summary)
        .collect();
    assert_eq!(reloaded, vec!["Configuration Reloaded".to_string()]);
    h.stop().await.expect("clean shutdown");
}

#[tokio::test(start_paused = true)]
async fn test_reload_missing_file_is_ignored() {
    let h = start(base_config(), FakeSwitcher::default(), Some("speakers"));
    h.config_changed.send(()).expect("loop running");
    tokio::time::sleep(Duration::from_secs(1)).await;
    let snapshot = h.snapshot().await;
    assert_eq!(h.reloads.load(Ordering::SeqCst), 0);
    assert_eq!(*snapshot.config, base_config());
    assert_eq!(h.notices(), vec![]);
    h.stop().await.expect("clean shutdown");
}

#[tokio::test(start_paused = true)]
async fn test_sighup_reload_changing_default_switches_with_no_windows() {
    let h = start(base_config(), FakeSwitcher::default(), Some("speakers"));
    let mut config = base_config();
    config.sinks[0].default = false;
    config.sinks[1].default = true;
    config.save_to(&h.config_path).expect("write config");

    h.signal(Signal::Hangup).await;
    h.wait_applied("hdmi").await;
    assert_eq!(h.reloads.load(Ordering::SeqCst), 1);
    h.stop().await.expect("clean shutdown");
}

#[tokio::test(start_paused = true)]
async fn test_invalid_reload_keeps_old_config() {
    let h = start(base_config(), FakeSwitcher::default(), Some("speakers"));
    std::fs::write(&h.config_path, "this is [not valid toml").expect("write");
    h.signal(Signal::Hangup).await;
    // Signals and queries use different channels: wait for the reload's outcome
    let notice = h.next_notice().await;
    assert_eq!(notice.summary, "Reload Failed");
    let snapshot = h.snapshot().await;
    assert_eq!(*snapshot.config, base_config());
    assert_eq!(h.reloads.load(Ordering::SeqCst), 0);
    assert_eq!(h.notices(), vec![]);
    h.stop().await.expect("clean shutdown");
}

// ----------------------------------------------------------------------------
// Shutdown
// ----------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn test_compositor_channel_close_is_an_error() {
    let mut h = start(base_config(), FakeSwitcher::default(), Some("speakers"));
    h.windows = None;
    let result = within("loop to exit", h.task).await.expect("loop task");
    let err = result.expect_err("losing the compositor must fail");
    assert!(format!("{err}").contains("Compositor connection lost"));
}

#[tokio::test(start_paused = true)]
async fn test_shutdown_command_and_signals_exit_cleanly() {
    let h = start(base_config(), FakeSwitcher::default(), Some("speakers"));
    h.command(Command::Shutdown).await;
    within("loop to exit", h.task)
        .await
        .expect("loop task")
        .expect("clean exit");

    let h = start(base_config(), FakeSwitcher::default(), Some("speakers"));
    h.signal(Signal::Interrupt).await;
    within("loop to exit", h.task)
        .await
        .expect("loop task")
        .expect("clean exit");
}

// ----------------------------------------------------------------------------
// Pause / resume
// ----------------------------------------------------------------------------

impl Harness {
    async fn set_paused(&self, request: PauseRequest) -> Response {
        let (reply, rx) = oneshot::channel();
        self.command(Command::Pause { request, reply }).await;
        within("pause reply", rx).await.expect("pause reply")
    }

    /// Pause and check the reply says so
    async fn pause(&self) {
        let response = self.set_paused(PauseRequest::Pause).await;
        assert!(
            matches!(response, Response::PauseState { paused: true, .. }),
            "got {response:?}"
        );
    }

    /// Resume and check the reply says so
    async fn resume(&self) {
        let response = self.set_paused(PauseRequest::Resume).await;
        assert!(
            matches!(response, Response::PauseState { paused: false, .. }),
            "got {response:?}"
        );
    }
}

/// Paused: a window that changes the rules' answer switches nothing; resuming
/// switches to the rules' answer at once
#[tokio::test(start_paused = true)]
async fn test_paused_rules_switch_nothing_until_resume() {
    let h = start(base_config(), FakeSwitcher::default(), Some("speakers"));
    h.pause().await;
    h.open(1, "firefox").await;
    let snapshot = h.wait_windows(1).await;
    assert!(snapshot.paused);
    assert_eq!(snapshot.desired, "headphones");
    assert_eq!(snapshot.tracked_windows, 1, "windows are still tracked");
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(h.calls(), Vec::<String>::new());
    assert_eq!(
        h.statuses().last().map(String::as_str),
        Some("Sink: Speakers (paused)")
    );

    h.resume().await;
    let snapshot = h.wait_applied("headphones").await;
    assert!(!snapshot.paused);
    assert_eq!(h.calls(), vec!["headphones"]);
    assert_eq!(
        h.statuses().last().map(String::as_str),
        Some("Sink: Headphones")
    );
    h.stop().await.expect("clean shutdown");
}

/// Manual switches still work while paused; resuming hands control back to the rules
#[tokio::test(start_paused = true)]
async fn test_manual_switch_works_while_paused() {
    let h = start(base_config(), FakeSwitcher::default(), Some("speakers"));
    h.pause().await;
    let response = h.switch(ManualSwitch::Set("HDMI".to_string())).await;
    assert!(
        matches!(&response, Response::SinkSwitched { desc, .. } if desc == "HDMI"),
        "got {response:?}"
    );
    h.open(1, "firefox").await;
    let snapshot = h.wait_windows(1).await;
    assert_eq!(snapshot.applied.as_deref(), Some("hdmi"));
    assert_eq!(h.calls(), vec!["hdmi"]);
    assert_eq!(
        h.statuses().last().map(String::as_str),
        Some("Sink: HDMI (paused)")
    );

    h.resume().await;
    h.wait_applied("headphones").await;
    assert_eq!(h.calls(), vec!["hdmi", "headphones"]);
    h.stop().await.expect("clean shutdown");
}

/// Resuming when the applied sink already is the rules' answer switches nothing,
/// even when the answer changed (and changed back) during the pause
#[tokio::test(start_paused = true)]
async fn test_resume_when_already_correct_switches_nothing() {
    let h = start(base_config(), FakeSwitcher::default(), Some("speakers"));
    h.pause().await;
    h.open(1, "firefox").await;
    h.close(1).await;
    h.open(2, "zsh").await;
    h.wait_windows(1).await;
    h.resume().await;
    tokio::time::sleep(Duration::from_secs(10)).await;
    let snapshot = h.snapshot().await;
    assert_eq!(snapshot.applied.as_deref(), Some("speakers"));
    assert_eq!(h.calls(), Vec::<String>::new());

    // Rules work as before afterwards
    h.open(3, "steam").await;
    h.wait_applied("hdmi").await;
    assert_eq!(h.calls(), vec!["hdmi"]);
    h.stop().await.expect("clean shutdown");
}

#[tokio::test(start_paused = true)]
async fn test_pause_resume_and_toggle_replies() {
    let h = start(base_config(), FakeSwitcher::default(), Some("speakers"));
    let state = |paused, changed| format!("{:?}", Response::PauseState { paused, changed });
    for (request, expected) in [
        (PauseRequest::Resume, state(false, false)),
        (PauseRequest::Toggle, state(true, true)),
        (PauseRequest::Pause, state(true, false)),
        (PauseRequest::Toggle, state(false, true)),
        (PauseRequest::Pause, state(true, true)),
        (PauseRequest::Resume, state(false, true)),
    ] {
        let response = h.set_paused(request).await;
        assert_eq!(format!("{response:?}"), expected, "{request:?}");
        let paused = matches!(response, Response::PauseState { paused: true, .. });
        assert_eq!(h.snapshot().await.paused, paused);
    }
    // Pause and resume were notified (notify_manual is on); repeats were not
    let summaries: Vec<String> = h.notices().into_iter().map(|n| n.summary).collect();
    assert_eq!(summaries, vec![
        "Switching Paused",
        "Switching Resumed",
        "Switching Paused",
        "Switching Resumed"
    ]);
    h.stop().await.expect("clean shutdown");
}

#[tokio::test(start_paused = true)]
async fn test_pause_is_not_notified_without_notify_manual() {
    let mut config = base_config();
    config.settings.notify_manual = false;
    let h = start(config, FakeSwitcher::default(), Some("speakers"));
    h.pause().await;
    h.resume().await;
    let _ = h.snapshot().await;
    assert_eq!(h.notices(), Vec::<Notice>::new());
    h.stop().await.expect("clean shutdown");
}

/// A pending retry is dropped by pausing; resuming tries again at once
#[tokio::test(start_paused = true)]
async fn test_pause_drops_pending_retry() {
    let h = start(
        base_config(),
        FakeSwitcher::failing("headphones", 1),
        Some("speakers"),
    );
    h.open(1, "firefox").await;
    h.wait_for("first attempt", |_, calls| calls.len() == 1)
        .await;
    h.pause().await;
    tokio::time::sleep(RETRY_BACKOFF[0] + RETRY_BACKOFF[1] + Duration::from_secs(1)).await;
    assert_eq!(h.calls(), vec!["headphones"], "no retry while paused");

    h.resume().await;
    h.wait_applied("headphones").await;
    assert_eq!(h.calls(), vec!["headphones"; 2]);
    h.stop().await.expect("clean shutdown");
}

/// Over `IPC`: the pause requests answer with the new state, and `Status` reports it
#[tokio::test]
async fn test_pause_over_ipc_and_status_field() {
    let h = start(base_config(), FakeSwitcher::default(), Some("speakers"));
    let status = serde_json::to_vec(&Request::Status).expect("serialize");
    let paused_in_status = |response: Response| match response {
        Response::Status { paused, .. } => paused,
        other => panic!("expected a status reply, got {other:?}"),
    };
    assert!(!paused_in_status(raw_ipc_exchange(&h, &status).await));

    for (request, paused) in [
        (Request::Pause, true),
        (Request::TogglePause, false),
        (Request::TogglePause, true),
    ] {
        let body = serde_json::to_vec(&request).expect("serialize");
        let response = raw_ipc_exchange(&h, &body).await;
        assert!(
            matches!(response, Response::PauseState { paused: p, changed: true } if p == paused),
            "{request:?}: got {response:?}"
        );
        assert_eq!(
            paused_in_status(raw_ipc_exchange(&h, &status).await),
            paused
        );
    }
    let body = serde_json::to_vec(&Request::Resume).expect("serialize");
    let response = raw_ipc_exchange(&h, &body).await;
    assert!(
        matches!(response, Response::PauseState {
            paused: false,
            changed: true
        }),
        "got {response:?}"
    );
    h.stop().await.expect("clean shutdown");
}

// ----------------------------------------------------------------------------
// Settle delay (`switch_delay_ms`)
// ----------------------------------------------------------------------------

const SETTLE: Duration = Duration::from_millis(300);

/// `base_config` with a 300 ms settle delay
fn settle_config() -> Config {
    let mut config = base_config();
    config.settings.switch_delay_ms = 300;
    config
}

/// A → B → A within the delay: no switch at all
#[tokio::test(start_paused = true)]
async fn test_settle_flicker_switches_nothing() {
    let h = start(settle_config(), FakeSwitcher::default(), Some("speakers"));
    h.open(1, "firefox").await;
    let snapshot = h.wait_windows(1).await;
    assert_eq!(snapshot.settling, Some(names(&["headphones"])));
    tokio::time::sleep(Duration::from_millis(100)).await;
    h.close(1).await;
    let snapshot = h.wait_windows(0).await;
    assert_eq!(
        snapshot.settling, None,
        "the change back cancelled the switch"
    );
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(h.calls(), Vec::<String>::new());
    assert_eq!(h.snapshot().await.applied.as_deref(), Some("speakers"));
    h.stop().await.expect("clean shutdown");
}

/// A → B held for the delay: switches once the delay has passed
#[tokio::test(start_paused = true)]
async fn test_settle_held_change_switches_after_delay() {
    let h = start(settle_config(), FakeSwitcher::default(), Some("speakers"));
    let t0 = Instant::now();
    h.open(1, "firefox").await;
    h.wait_windows(1).await;
    tokio::time::sleep(Duration::from_millis(290)).await;
    assert_eq!(h.calls(), Vec::<String>::new(), "not before the delay");
    h.wait_applied("headphones").await;
    assert!(t0.elapsed() >= SETTLE, "waited {:?}", t0.elapsed());
    assert_eq!(h.calls(), vec!["headphones"]);
    h.stop().await.expect("clean shutdown");
}

/// B → C during the wait: only C switches, a full delay after the last change
#[tokio::test(start_paused = true)]
async fn test_settle_restarts_on_further_change() {
    let h = start(settle_config(), FakeSwitcher::default(), Some("speakers"));
    let t0 = Instant::now();
    h.open(1, "firefox").await;
    h.wait_windows(1).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let changed_at = Instant::now();
    h.open(2, "steam").await;
    let snapshot = h.wait_windows(2).await;
    assert_eq!(snapshot.settling, Some(names(&["hdmi"])));
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(h.calls(), Vec::<String>::new(), "B never switched");
    h.wait_applied("hdmi").await;
    assert!(
        changed_at.elapsed() >= SETTLE,
        "switched {:?} after the last change",
        changed_at.elapsed()
    );
    assert!(t0.elapsed() >= Duration::from_millis(500));
    assert_eq!(h.calls(), vec!["hdmi"]);
    h.stop().await.expect("clean shutdown");
}

/// Delay 0 (the default): rule-driven switches are submitted at once
#[tokio::test(start_paused = true)]
async fn test_no_settle_delay_switches_immediately() {
    let h = start(base_config(), FakeSwitcher::default(), Some("speakers"));
    let t0 = Instant::now();
    h.open(1, "firefox").await;
    let snapshot = h.wait_windows(1).await;
    assert_eq!(snapshot.settling, None);
    h.wait_applied("headphones").await;
    assert!(t0.elapsed() < SETTLE, "took {:?}", t0.elapsed());
    h.stop().await.expect("clean shutdown");
}

/// Manual requests are not delayed, and one made during the wait holds
#[tokio::test(start_paused = true)]
async fn test_settle_manual_switch_is_immediate_and_holds() {
    let h = start(settle_config(), FakeSwitcher::default(), Some("speakers"));
    h.open(1, "firefox").await;
    h.wait_windows(1).await;
    let response = h.switch(ManualSwitch::Set("HDMI".to_string())).await;
    assert!(
        matches!(&response, Response::SinkSwitched { desc, .. } if desc == "HDMI"),
        "got {response:?}"
    );
    tokio::time::sleep(Duration::from_secs(5)).await;
    let snapshot = h.snapshot().await;
    assert_eq!(snapshot.applied.as_deref(), Some("hdmi"));
    assert_eq!(snapshot.settling, None);
    assert_eq!(h.calls(), vec!["hdmi"]);
    h.stop().await.expect("clean shutdown");
}

/// Startup activation and resume are not delayed; pausing cancels a pending wait
#[tokio::test(start_paused = true)]
async fn test_settle_not_applied_to_startup_and_resume() {
    let mut config = settle_config();
    config.settings.default_on_startup = true;
    let h = start(config, FakeSwitcher::default(), Some("hdmi"));
    let t0 = Instant::now();
    h.wait_applied("speakers").await;
    assert!(t0.elapsed() < SETTLE, "startup took {:?}", t0.elapsed());

    h.open(1, "firefox").await;
    h.wait_windows(1).await;
    h.pause().await;
    assert_eq!(
        h.snapshot().await.settling,
        None,
        "pausing cancels the wait"
    );
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(h.calls(), vec!["speakers"]);

    let t1 = Instant::now();
    h.resume().await;
    h.wait_applied("headphones").await;
    assert!(t1.elapsed() < SETTLE, "resume took {:?}", t1.elapsed());
    h.stop().await.expect("clean shutdown");
}

// ----------------------------------------------------------------------------
// Sources (microphones)
// ----------------------------------------------------------------------------

/// `base_config` plus sources Desk Mic (default) and Headset Mic, Arctis Mic; rules:
/// discord → headphones + Headset Mic, zoom → speakers + [Arctis Mic, Headset Mic]
fn source_config() -> Config {
    let mut config = base_config().with_sources(vec![
        make_sink("mic.desk", "Desk Mic", true),
        make_sink("mic.headset", "Headset Mic", false),
        make_sink("mic.arctis", "Arctis Mic", false),
    ]);
    config
        .rules
        .push(with_sources(make_rule("^discord$", None, "headphones"), &[
            "Headset Mic",
        ]));
    config
        .rules
        .push(with_sources(make_rule("^zoom$", None, "speakers"), &[
            "Arctis Mic",
            "Headset Mic",
        ]));
    config
}

fn with_desk_mic(switcher: FakeSwitcher) -> FakeSwitcher {
    switcher.set_current_source("mic.desk");
    switcher
}

/// No sources configured: pwsw never asks for a source, and the status line has no mic
#[tokio::test(start_paused = true)]
async fn test_no_sources_never_touches_the_default_source() {
    let h = start(
        base_config(),
        with_desk_mic(FakeSwitcher::default()),
        Some("speakers"),
    );
    h.open(1, "firefox").await;
    h.wait_applied("headphones").await;
    h.close(1).await;
    let snapshot = h.wait_applied("speakers").await;
    assert!(!snapshot.sources_configured);
    assert_eq!(snapshot.applied_source, None);
    assert_eq!(h.source_requests(), Vec::new());
    assert!(h.halves().iter().all(|&(_, source)| !source));
    assert_eq!(
        h.statuses().last().map(String::as_str),
        Some("Sink: Speakers")
    );
    let response = h
        .switch(ManualSwitch::SetSource("Desk Mic".to_string()))
        .await;
    let Response::Error { message } = response else {
        panic!("expected an error, got {response:?}");
    };
    assert!(message.starts_with("No sources configured"), "{message}");
    h.stop().await.expect("clean shutdown");
}

/// A rule with a source switches both, jointly; a sink-only rule after it moves
/// only the sink, and the microphone stays with the window that chose it
#[tokio::test(start_paused = true)]
async fn test_source_follows_its_own_rules_independently() {
    let h = start(
        source_config(),
        with_desk_mic(FakeSwitcher::default()),
        Some("speakers"),
    );
    h.open(1, "discord").await;
    h.wait_source("mic.headset").await;
    let snapshot = h.wait_applied("headphones").await;
    assert_eq!(h.halves(), vec![(true, true)], "one joint activation");
    assert_eq!(snapshot.desired_source.as_deref(), Some("mic.headset"));
    assert_eq!(
        h.statuses().last().map(String::as_str),
        Some("Sink: Headphones · Mic: Headset Mic")
    );
    let discord = snapshot
        .windows
        .iter()
        .find(|w| w.id == Some(1))
        .expect("window");
    let tracked = discord.tracked.as_ref().expect("tracked");
    assert_eq!(tracked.sources[0].desc, "Headset Mic");

    // Steam: sink only; the microphone answer is unchanged, so no source job
    h.open(2, "steam").await;
    let snapshot = h.wait_applied("hdmi").await;
    assert_eq!(snapshot.applied_source.as_deref(), Some("mic.headset"));
    assert_eq!(h.source_calls(), vec!["mic.headset"]);
    assert_eq!(h.halves(), vec![(true, true), (true, false)]);

    // Discord closes: the sink goes to Steam's, the mic back to the default
    h.close(1).await;
    h.wait_source("mic.desk").await;
    assert_eq!(
        h.calls(),
        vec!["headphones", "hdmi"],
        "sink stays on Steam's"
    );
    assert_eq!(h.source_calls(), vec!["mic.headset", "mic.desk"]);
    h.stop().await.expect("clean shutdown");
}

/// A source rule with fallbacks settles for the next reachable one, and upgrades
/// when the first choice comes back (on the next event)
#[tokio::test(start_paused = true)]
async fn test_source_fallback_and_upgrade() {
    let switcher = with_desk_mic(FakeSwitcher::default());
    switcher.set_absent(&["mic.arctis"]);
    let h = start(source_config(), switcher, Some("speakers"));
    h.open(1, "zoom").await;
    let snapshot = h.wait_source("mic.headset").await;
    assert!(snapshot.source_fallback);
    assert_eq!(h.source_requests(), vec![(
        names(&["mic.arctis", "mic.headset"]),
        None
    )]);
    assert!(
        h.statuses()
            .last()
            .is_some_and(|s| s.ends_with("Mic: Headset Mic (fallback)")),
        "{:?}",
        h.statuses()
    );

    h.switcher.set_absent(&[]);
    tokio::time::sleep(UPGRADE_CHECK_INTERVAL).await;
    h.open(2, "zsh").await;
    let snapshot = h.wait_source("mic.arctis").await;
    assert!(!snapshot.source_fallback);
    assert_eq!(
        h.source_requests().last(),
        Some(&(names(&["mic.arctis", "mic.headset"]), Some(1)))
    );
    h.stop().await.expect("clean shutdown");
}

/// A failed source switch retries on its own schedule; the sink is not resubmitted
#[tokio::test(start_paused = true)]
async fn test_failed_source_switch_retries_without_the_sink() {
    let h = start(
        source_config(),
        with_desk_mic(FakeSwitcher::failing("mic.headset", 1)),
        Some("speakers"),
    );
    h.open(1, "discord").await;
    h.wait_source("mic.headset").await;
    assert_eq!(h.source_calls(), vec!["mic.headset", "mic.headset"]);
    assert_eq!(h.calls(), vec!["headphones"], "the sink switched once");
    assert_eq!(h.halves(), vec![(true, true), (false, true)]);
    h.stop().await.expect("clean shutdown");
}

/// Pausing stops source switching too; resuming adopts both answers
#[tokio::test(start_paused = true)]
async fn test_pause_stops_source_switching() {
    let h = start(
        source_config(),
        with_desk_mic(FakeSwitcher::default()),
        Some("speakers"),
    );
    h.pause().await;
    h.open(1, "discord").await;
    let snapshot = h.wait_windows(1).await;
    assert_eq!(snapshot.desired_source.as_deref(), Some("mic.headset"));
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(h.halves(), Vec::new());
    assert_eq!(
        h.statuses().last().map(String::as_str),
        Some("Sink: Speakers · Mic: Desk Mic (paused)")
    );
    h.resume().await;
    h.wait_source("mic.headset").await;
    h.wait_applied("headphones").await;
    h.stop().await.expect("clean shutdown");
}

/// The settle delay runs per dimension: a sink flicker back to the applied sink
/// cancels the sink switch, while the source change that held still switches
#[tokio::test(start_paused = true)]
async fn test_settle_delay_is_per_dimension() {
    let mut config = source_config();
    config.settings.switch_delay_ms = 300;
    let h = start(
        config,
        with_desk_mic(FakeSwitcher::default()),
        Some("speakers"),
    );
    h.open(1, "discord").await;
    let snapshot = h.wait_windows(1).await;
    assert_eq!(snapshot.settling, Some(names(&["headphones"])));
    assert_eq!(snapshot.source_settling, Some(names(&["mic.headset"])));
    // Within the delay a zoom window opens: sink back to speakers (cancels the sink
    // wait), source to Arctis (restarts the source wait)
    tokio::time::sleep(Duration::from_millis(100)).await;
    h.open(2, "zoom").await;
    let snapshot = h.wait_windows(2).await;
    assert_eq!(snapshot.settling, None, "the sink is already right");
    assert_eq!(
        snapshot.source_settling,
        Some(names(&["mic.arctis", "mic.headset"]))
    );
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(
        h.source_calls(),
        Vec::<String>::new(),
        "not before its delay"
    );
    h.wait_source("mic.arctis").await;
    assert_eq!(h.calls(), Vec::<String>::new(), "no sink switch at all");
    assert_eq!(h.halves(), vec![(false, true)]);
    h.stop().await.expect("clean shutdown");
}

/// `set-source` holds until the rules' source answer changes; sink changes do not end
/// it, and a manual sink choice does not end a source rule either
#[tokio::test(start_paused = true)]
async fn test_manual_set_source_holds_until_the_source_answer_changes() {
    let h = start(
        source_config(),
        with_desk_mic(FakeSwitcher::default()),
        Some("speakers"),
    );
    let response = h
        .switch(ManualSwitch::SetSource("Arctis Mic".to_string()))
        .await;
    assert!(
        matches!(&response, Response::SinkSwitched { desc, node_name }
            if desc == "Arctis Mic" && node_name == "node.mic.arctis"),
        "{response:?}"
    );
    let snapshot = h.wait_source("mic.arctis").await;
    assert!(snapshot.source_manual_override);
    assert!(!snapshot.manual_override, "the sink lane is untouched");

    // A sink-only change of the rules: the manual microphone stays
    h.open(1, "firefox").await;
    let snapshot = h.wait_applied("headphones").await;
    assert!(snapshot.source_manual_override);
    assert_eq!(snapshot.applied_source.as_deref(), Some("mic.arctis"));

    // Smart toggle: asking for the active source goes back to the default
    let response = h
        .switch(ManualSwitch::SetSource("Arctis Mic".to_string()))
        .await;
    assert!(
        matches!(&response, Response::SinkSwitched { desc, .. } if desc == "Desk Mic"),
        "{response:?}"
    );
    h.wait_source("mic.desk").await;
    let response = h
        .switch(ManualSwitch::SetSource("Headset Mic".to_string()))
        .await;
    assert!(
        matches!(response, Response::SinkSwitched { .. }),
        "{response:?}"
    );

    // The source answer changes (discord): the rules take over the mic again
    h.open(2, "discord").await;
    let snapshot = h
        .wait_for("discord's mic and sink", |s, _| {
            s.applied.as_deref() == Some("headphones")
                && s.applied_source.as_deref() == Some("mic.headset")
                && !s.source_manual_override
        })
        .await;
    assert_eq!(snapshot.desired_source.as_deref(), Some("mic.headset"));
    let unknown = h.switch(ManualSwitch::SetSource("Nope".to_string())).await;
    assert!(
        matches!(&unknown, Response::Error { message } if message.contains("Unknown source 'Nope'")),
        "{unknown:?}"
    );
    h.stop().await.expect("clean shutdown");
}

/// Sources added by a reload become a managed dimension; removing them stops it
#[tokio::test(start_paused = true)]
async fn test_reload_adds_and_removes_the_source_lane() {
    let h = start(
        base_config(),
        with_desk_mic(FakeSwitcher::default()),
        Some("speakers"),
    );
    h.open(1, "discord").await;
    h.wait_windows(1).await;
    assert_eq!(h.source_requests(), Vec::new());

    let with_sources = "[[sinks]]\nname = \"speakers\"\ndesc = \"Speakers\"\ndefault = true\n\n\
        [[sinks]]\nname = \"headphones\"\ndesc = \"Headphones\"\n\n\
        [[sources]]\nname = \"mic.desk\"\ndesc = \"Desk Mic\"\ndefault = true\n\n\
        [[sources]]\nname = \"mic.headset\"\ndesc = \"Headset Mic\"\n\n\
        [[rules]]\napp_id = \"^discord$\"\nsink = \"Headphones\"\nsource = \"Headset Mic\"\n";
    std::fs::write(&h.config_path, with_sources).expect("write config");
    h.signal(Signal::Hangup).await;
    let snapshot = h.wait_source("mic.headset").await;
    assert!(snapshot.sources_configured);
    h.wait_applied("headphones").await;

    let without = "[[sinks]]\nname = \"speakers\"\ndesc = \"Speakers\"\ndefault = true\n\n\
        [[sinks]]\nname = \"headphones\"\ndesc = \"Headphones\"\n\n\
        [[rules]]\napp_id = \"^discord$\"\nsink = \"Headphones\"\n";
    std::fs::write(&h.config_path, without).expect("write config");
    h.signal(Signal::Hangup).await;
    let snapshot = h
        .wait_for("sources gone", |s, _| !s.sources_configured)
        .await;
    assert_eq!(snapshot.applied_source, None);
    h.close(1).await;
    h.wait_applied("speakers").await;
    assert_eq!(
        h.source_calls(),
        vec!["mic.headset"],
        "no source switch after removal"
    );
    assert_eq!(
        h.statuses().last().map(String::as_str),
        Some("Sink: Speakers")
    );
    h.stop().await.expect("clean shutdown");
}

// ----------------------------------------------------------------------------
// Monitor (`output`) rules
// ----------------------------------------------------------------------------

const EXT: &str = "ext_foreign_toplevel_list_v1";
const WLR: &str = "zwlr_foreign_toplevel_manager_v1";

/// A compositor connection on `interface`, spawned with `preference`
fn compositor_on(interface: &str, preference: ProtocolPreference) -> CompositorInfo {
    CompositorInfo {
        capabilities: detected_capabilities([interface], preference).expect("supported"),
        preference,
    }
}

/// [`base_config`] plus: any window on `HDMI-A-1` → hdmi (first rule)
fn monitor_config() -> Config {
    let mut config = base_config();
    config
        .rules
        .insert(0, with_output(make_rule("", None, "hdmi"), "^HDMI-A-1$"));
    config
}

/// Capture what this thread logs at `WARN` and above while the guard lives (the
/// harness runs the loop on the test's single-threaded runtime)
struct LogCapture {
    buffer: Arc<Mutex<Vec<u8>>>,
    _guard: tracing::subscriber::DefaultGuard,
}

impl LogCapture {
    fn start() -> Self {
        #[derive(Clone)]
        struct Writer(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Writer {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().expect("lock").extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let writer = Writer(Arc::clone(&buffer));
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::WARN)
            .finish();
        Self {
            buffer,
            _guard: tracing::subscriber::set_default(subscriber),
        }
    }

    fn text(&self) -> String {
        String::from_utf8(self.buffer.lock().expect("lock").clone()).expect("utf-8 logs")
    }
}

/// A window moving onto the monitor an `output` rule names switches; moving off
/// switches back to the default
#[tokio::test(start_paused = true)]
async fn test_placement_event_switches() {
    let h = start(monitor_config(), FakeSwitcher::default(), Some("speakers"));
    h.open(1, "vlc").await;
    h.event(WindowEvent::Placement {
        id: 1,
        outputs: vec!["HDMI-A-1".to_string()],
        minimized: false,
    })
    .await;
    let snapshot = h.wait_applied("hdmi").await;
    assert_eq!(snapshot.windows[0].outputs, ["HDMI-A-1"]);
    assert!(!snapshot.windows[0].minimized);
    assert!(snapshot.windows[0].tracked.is_some());

    h.event(WindowEvent::Placement {
        id: 1,
        outputs: vec!["HDMI-A-1".to_string()],
        minimized: true,
    })
    .await;
    let snapshot = h.wait_applied("speakers").await;
    assert!(snapshot.windows[0].minimized);
    assert_eq!(h.calls(), vec!["hdmi", "speakers"]);
    h.stop().await.expect("clean shutdown");
}

/// Output rules added by a reload while the daemon runs on ext: it warns that a
/// restart is needed (nothing is reconnected); no warning on wlr
#[tokio::test(start_paused = true)]
async fn test_reload_adding_output_rules_on_ext_warns() {
    for (compositor, warns) in [
        (compositor_on(EXT, ProtocolPreference::Auto), true),
        (compositor_on(WLR, ProtocolPreference::Auto), false),
    ] {
        let logs = LogCapture::start();
        let h = start_with(
            base_config(),
            FakeSwitcher::default(),
            Some("speakers"),
            Some(compositor),
        );
        monitor_config()
            .save_to(&h.config_path)
            .expect("write config");
        h.signal(Signal::Hangup).await;
        h.wait_for("reload", |s, _| s.config.has_output_rules())
            .await;
        let text = logs.text();
        assert_eq!(
            text.contains("restart pwsw"),
            warns,
            "{compositor:?}: {text}"
        );
        h.stop().await.expect("clean shutdown");
    }
}

#[test]
fn test_output_rules_warning() {
    let ext_auto = compositor_on(EXT, ProtocolPreference::Auto);
    let ext_only = compositor_on(EXT, ProtocolPreference::PreferWlr);
    let wlr = compositor_on(WLR, ProtocolPreference::PreferWlr);
    assert_eq!(output_rules_warning(ext_auto, false), None);
    assert_eq!(output_rules_warning(wlr, true), None);
    assert_eq!(output_rules_warning(wlr, false), None);
    let restart = output_rules_warning(ext_auto, true).expect("warns");
    assert!(restart.contains("restart pwsw"), "{restart}");
    let never = output_rules_warning(ext_only, true).expect("warns");
    assert!(
        never.contains("need wlr-foreign-toplevel-management"),
        "{never}"
    );
    assert!(never.contains("only offers ext"), "{never}");
    assert!(never.contains("will never match"), "{never}");
}

/// `Status` names the backend and whether monitor info is available
#[tokio::test]
async fn test_status_reports_backend_and_monitor_info() {
    let h = start(base_config(), FakeSwitcher::default(), Some("speakers"));
    let body = serde_json::to_vec(&Request::Status).expect("serialize");
    for (compositor, expected) in [
        (None, (None, false)),
        (
            Some(compositor_on(WLR, ProtocolPreference::PreferWlr).capabilities),
            (Some("wlr"), true),
        ),
        (
            Some(compositor_on(EXT, ProtocolPreference::Auto).capabilities),
            (Some("ext"), false),
        ),
    ] {
        let response = raw_ipc_exchange_with(&h, &body, compositor).await;
        let Response::Status {
            backend,
            monitor_info,
            ..
        } = response
        else {
            panic!("expected a status reply, got {response:?}");
        };
        assert_eq!((backend.as_deref(), monitor_info), expected);
    }
    h.stop().await.expect("clean shutdown");
}

mod status_stream;
