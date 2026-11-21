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
use crate::daemon::ipc_handler::{ServerInfo, handle_connection};
use crate::daemon_manager::DaemonManager;
use crate::ipc::Request;
use crate::test_utils::fixtures::{make_config, make_rule, make_rule_with_fallbacks, make_sink};

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
    /// Every `activate` request: the candidates and `upgrade_from`
    requests: Mutex<Vec<(Vec<String>, Option<usize>)>>,
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
    fn activate_one(&self, sink_name: &str) -> Result<String> {
        self.calls.lock().expect("lock").push(sink_name.to_string());
        if let Some(tx) = &*self.started.lock().expect("lock") {
            let _ = tx.send(sink_name.to_string());
        }
        if let Some(gate) = &*self.gate.lock().expect("lock") {
            // Bounded: a held activation must never keep runtime shutdown waiting
            let _ = gate.recv_timeout(WAIT);
        }
        let mut failures = self.failures.lock().expect("lock");
        if let Some(left) = failures.get_mut(sink_name)
            && *left > 0
        {
            *left -= 1;
            eyre::bail!("fake failure for {sink_name}");
        }
        *self.current.lock().expect("lock") = Some(sink_name.to_string());
        Ok(format!("node.{sink_name}"))
    }
}

impl SinkSwitcher for FakeSwitcher {
    fn activate(
        &self,
        candidates: &[String],
        upgrade_from: Option<usize>,
    ) -> Result<Option<(usize, String)>> {
        self.requests
            .lock()
            .expect("lock")
            .push((candidates.to_vec(), upgrade_from));
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
            match self.activate_one(candidate) {
                Ok(node) => return Ok(Some((index, node))),
                Err(e) => errors.push(format!("{e:#}")),
            }
        }
        eyre::bail!("{}", errors.join("; "))
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
    let switcher = Arc::new(switcher);
    let daemon = Daemon::new(
        State::new(Arc::new(config)).expect("valid config"),
        config_path.clone(),
        Arc::clone(&switcher),
        notifier,
        hooks,
        applied.map(str::to_string),
    );
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
    let (mut client, server) = tokio::net::UnixStream::pair().expect("socket pair");
    let commands = h.commands.clone();
    let server_task = tokio::spawn(async move {
        let info = ServerInfo {
            version: "v9.9.9+test".to_string(),
            started: std::time::Instant::now(),
            daemon_manager: DaemonManager::Direct,
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
