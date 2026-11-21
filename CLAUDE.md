# CLAUDE.md

Guidance for coding agents working on this repository. If the code contradicts this
file, the code wins; fix the file in the same change.

## What pwsw is

A Rust daemon that sets the PipeWire default sink (and, with `[[sources]]` configured,
the default source) from rules that match Wayland windows
(`ext-foreign-toplevel-list-v1` or `wlr-foreign-toplevel-management`), plus a CLI that
talks to it over a Unix socket and a `ratatui` TUI for configuration and monitoring.
Single crate, library + one binary (`src/bin/pwsw.rs`); the library's only consumer is
that binary.

## Module map

| Path | Owns |
|------|------|
| `src/bin/pwsw.rs` | Entry point: dispatch, error printing, exit codes (0 ok / 1 error / 2 usage) |
| `src/lib.rs` | Module list, `VERSION`/`version_string()` (`v<version>+<hash>`), `format_uptime`; `tui` only with the default `tui` feature |
| `src/cli.rs` | `clap` definitions (`--config`, `--color`, subcommands, help text) |
| `src/commands.rs` | CLI commands (`status` incl. `--json`, `list-*`, `test-rule` incl. `--output`, `validate`, manual switches, `pause`/`resume`) |
| `src/doctor.rs` | `pwsw doctor`: read-only checks (Wayland protocols, `PipeWire` tools, config, sinks/mics, daemon, systemd unit, log), ✓/·/⚠/✗ report |
| `src/status_watch.rs` | `status --watch [--json]`: follows the daemon's `Subscribe` stream, bar `JSON` lines, reconnect with backoff |
| `src/config.rs` | Schema, path selection, validation, all mutations, comment-preserving atomic saves, `init` |
| `src/state.rs` | Pure reducer: window events → tracked windows and rule matches; `desired()`, `desired_for(Endpoint)` |
| `src/manual.rs` | Pure decision for `set-sink`/`next-sink`/`prev-sink`/`set-source` against a PipeWire probe (daemon and CLI fallback) |
| `src/daemon.rs` | Daemon startup: lock, detach, logging, compositor, IPC server, config watcher, wiring |
| `src/daemon/` | `event_loop.rs` (the `select!` loop: reconcile, reload, retries, `sd_notify`), `worker.rs` (activations), `ipc_handler.rs` (per-connection tasks), `notifier.rs` (notification thread), `lock.rs` (`flock` instance lock) |
| `src/daemon_manager.rs` | systemd vs direct detection (`/proc/self/cgroup`), `systemctl` queries with timeouts |
| `src/ipc.rs` | Socket path, peer checks, framing, stale cleanup, `PROTOCOL_VERSION` |
| `src/compositor/` | `mod.rs` (Wayland thread, protocol detection, `ProtocolProbe`, `WindowEvent`), `tracker.rs` (`WindowTracker`, `EventOutbox` coalescing), `ext_toplevel.rs`, `wlr_toplevel.rs`, `output.rs` (`wl_output` connector names, `wlr` only) |
| `src/pipewire/` | `model`, `parse` (pure), `resolve` (pure), `activate` (over `AudioBackend` + `Clock`), `cli` (only process spawning), `node_state` (`NodeState`: current / present / via profile / missing, shared by `list-sinks` and the TUI) |
| `src/notification.rs` | `notify-rust` wrapper, markup capability detection, icon selection |
| `src/sanitize.rs` | Untrusted text for logs, notifications, captured child output |
| `src/logging.rs` | Size-rotated `daemon.log` appender (1 MB, one `.old`, synchronous writes), reloadable level filter, panic hook that records panics in the log |
| `src/style.rs` | `PwswStyle`/`Painted`, per-stream colour decision, non-panicking `outln!`/`errln!` |
| `src/tui/` | `mod.rs` (loop), `app.rs` (state), `input.rs` (key handling, editors), `keymap.rs` (binding table), `worker.rs` (background polling and tasks), `daemon_control.rs`, `log_tailer.rs`, `preview.rs` (rule live preview), `terminal.rs` (raw mode/alternate screen, restored once), `chrome.rs` (one-row header and footer), `theme.rs` (every style; colourless with `NO_COLOR`), `widgets.rs` (dialogs, forms, text helpers), `screens/` (`list.rs`: the list/form/dialog code Sinks and Rules share; `dashboard`, `sinks`, `rules`, `settings`, `help`), `tests/` |
| `src/test_utils.rs` | Test fixtures (`make_config`, `make_sink`, `make_rule`) |
| `build.rs` | Man pages from `man/*.md` (`mandown`) into `OUT_DIR`; `PWSW_VERSION` (`<version>+<hash>[-dirty]`) |
| `tests/` | CLI smoke tests (sandboxed binary, incl. config lifecycle), mock-compositor tests (`ext` and `wlr`), `pw-dump` fixtures (`tests/fixtures/README.md`) |
| `contrib/` | `PKGBUILD`, systemd user unit, desktop entry, icons, `noctalia/` bar plugin (untested by CI) |
| `scripts/` | `verify_tests_safe.sh` (sandboxed test run), `install_git_hook.sh` |
| `docs/` | Local planning notes, gitignored; never commit or rely on it |

## Architecture you must not break

**Decision is pure, acting is asynchronous.** `State::apply` folds `WindowEvent`s into
the window set; `State::desired()` implements both priority modes (most recent window by
a monotonic sequence number, renewed when a window starts matching; or lowest rule index
with `match_by_index`, ties to the most recent), falling back to the default sink. Its
answer is an ordered candidate list: the winning rule's sink and fallbacks
(`sink = ["A", "B"]`), or the default sink alone. Each window is matched by the first
rule that matches it (`Rule::matches`: every condition the rule sets — `app_id`, `title`,
`output` — must match; `output` matches when some connector name of the window's monitors
matches and the window is not minimized). `State` keeps each window's `outputs` and
`minimized` from `WindowEvent::Placement`; a placement change that makes a window start
matching renews its sequence number like a title change; staying matched keeps it. Windows
from the `ext` backend never get a placement, so `output` rules never match them. Nothing
in `state.rs` does I/O; reachability is the worker's call.

**Sources are a second, independent dimension** (`config::Endpoint::{Sink, Source}`).
`State::desired_for(Source)` applies the same priority but only over matching windows
whose rule has a `source` (else the default source; `None` without `[[sources]]`, and
then nothing ever touches the default source). The event loop keeps one `Lane` per
endpoint (applied, last answer, retries, settle, upgrade check, manual override,
superseded job); every rule below holds per lane, and a lane never looks at the other.
Pause stops both. Shared code takes an `Endpoint` instead of being duplicated.

**Switch only when the rules' answer changes.** After every input the event loop
reconciles: if `desired()` differs from the previous answer (or a switch the rules still
want failed), it submits a job. It never compares against the real PipeWire default, so a
sink chosen in `pavucontrol` or by WirePlumber stays until the rules want something else.
At startup the loop first applies the windows the compositor already has (up to
`WindowEvent::InitialSync`, at most `INITIAL_SYNC_TIMEOUT`, 2 s), then takes the answer
as the baseline; with `default_on_startup` it is activated. Window events already queued
when the loop wakes are applied together (up to `WINDOW_EVENT_BATCH`) before one
reconcile. Failed rule switches retry after 2 s and 5 s, then once more on the next
event or reload. A manual switch holds the same way; a failed manual switch changes
nothing. "Changed" means
the candidate list changed. A rule job activates the first reachable candidate against
one snapshot (`pipewire::activate::activate_first`), trying the next reachable one if
that activation fails; when it settled for a fallback, the next window event or reload
(at most every `UPGRADE_CHECK_INTERVAL`, 2 s, never polled, never over a manual choice)
submits an *upgrade* job that switches only if an earlier candidate is reachable. An
applied fallback still *serves* the list it was activated for (`Lane::applied_for`), so
the rules returning to that list switch nothing; a failed upgrade check is never retried
and a superseded one is never resumed (either would be a full rule switch over whatever
the user picked since).

**Pause and settle delay.** `Pause`/`Resume`/`TogglePause` set an in-memory `paused`
flag (a restart starts unpaused): windows, reloads and manual requests keep working, but
no rule-driven job, retry or upgrade is submitted, and pausing drops pending retries and
settle waits. Resume adopts the rules' current answer like a change (switches unless it
is already served, ends a manual choice). With `switch_delay_ms` > 0 a change of the
answer arms a `Settle` timer instead of submitting; it fires only if `last_desired` is
still that list, another change re-arms it, a change back to what is served cancels it.
Manual requests, retries, upgrades, startup and resume are never delayed.

**Never await PipeWire in the `select!`.** The loop submits to the worker
(`daemon/worker.rs`), which runs activations one at a time in `spawn_blocking`; results
come back as another input. The worker's `watch` channel has one slot per endpoint
("latest wins" per slot); the loop queues jobs in an outbox flushed once per loop
iteration, so a sink job and a source job of the same step reach the worker together
and run as one joint activation (`pipewire::activate::activate_joint`). IPC, signals and window events must keep flowing during a
slow profile switch. Manual requests are planned in the worker against a fresh probe
(`manual::plan`), not on the loop. Notifications go to the notifier thread (`try_send`,
never wait).

**PipeWire only through its CLI tools** (`pw-dump`, `pw-metadata`, `pw-cli`), spawned only
from `pipewire/cli.rs`. Every call has a hard deadline (`PW_DUMP_TIMEOUT` 5 s,
`PW_COMMAND_TIMEOUT` 3 s; the child is killed and reaped), an output cap and an
exit-status check. One process-wide activation lock; snapshot and decision are taken
inside it.
- Identity: configs store node names. A predicted name `alsa_output.<card>.<mapping>` only
  finds *candidates*; after a profile switch the node is recognised by `device.id` plus
  `device.profile.name` (or `card.profile.device`), never by name (WirePlumber may rename
  nodes). Profile choice keeps the card's current input mapping when possible; only
  available profiles are used; UCM (`HiFi`) and `pro-audio` are never switched to.
- The default is written to `default.configured.audio.sink` /
  `default.configured.audio.source` (like `wpctl set-default`), never
  `default.audio.sink`/`.source`. Profile switches are sent with `save: true`.
- Sources: `Audio/Source` nodes exactly (`*.monitor` and `Audio/Source/Virtual` are
  skipped), predicted `alsa_input.<card>.<mapping>` from `input:` profile parts, same
  identity matching. A source-only switch must keep the card's current outputs.
- Joint jobs (`resolve::plan_joint`): one snapshot for both halves; the sink is chosen as
  on its own. If the sink's card switch is on the source's card, one profile providing
  both mappings replaces the sink's profile (the card switches once); if none exists the
  sink wins and that source candidate is skipped as unreachable. A source switch on the
  sink's card must keep the sink's output. A job never switches a card twice for its
  two halves (cards switched for the sink are frozen for the source's fallthrough).
- Post-switch wait: `PROFILE_SWITCH_DELAY_MS` × `PROFILE_SWITCH_MAX_RETRIES`, capped at 5 s.
- `activate.rs` runs over the `AudioBackend` and `Clock` traits so the whole flow is
  tested with fakes; put new logic there or in `resolve.rs`, not in `cli.rs`.

**Config layer invariants** (`config.rs`):
- All mutations go through `Config` methods (`add_sink`, `update_sink`, `set_default_sink`,
  `remove_sink`, `move_sink`, the same five for sources (`add_source`, ...), `add_rule`,
  `update_rule`, `remove_rule`, `move_rule`). They run on a clone, validate, then commit;
  renames and moves keep rules pointing at the same sink/source; one still referenced by
  rules (as first choice or fallback) cannot be removed. Sink and source variants share
  the generic `*_endpoint(Endpoint, ..)` implementations. The TUI uses only these.
- `[[sources]]` is optional; when present exactly one is `default`. A rule's
  `Rule::source_refs` (ordered, empty = no source) may only be non-empty when sources
  exist; refs resolve, cascade and round-trip exactly like sink refs (`source` key kept
  as written when unchanged). `update_rule` replaces `source_refs` too: a caller that
  does not edit sources must pass the old ones along (the TUI's `RuleEditor` carries
  them, and `output_pattern`, through unchanged).
- A rule needs `app_id` or `output` (both optional in the file). No `app_id` is stored as
  an empty `Rule::app_id_pattern` (`Rule::has_app_id`; `app_id = ""` is rejected at load,
  and a rule that is empty with no `output` by validation); an empty `output` is rejected
  too. Saves omit `app_id` for such rules. Use `Rule::label()` to name a rule in
  messages.
- A rule's targets are `Rule::sink_ref` (first choice) plus `Rule::fallback_refs`
  (`Rule::sink_refs()` yields both, in order); cascades cover every entry. `add_rule` /
  `update_rule` store all of them as `desc`s and drop a fallback equal to the first
  choice (the TUI edits only the first choice and passes the old fallbacks along). A rule
  with one sink saves as a string, one with several as an inline array; unchanged values
  are left exactly as written.
- Every save validates and refuses text that does not load back to the identical config.
- Saves apply values into the loaded `toml_edit` document, so comments and formatting of
  untouched entries survive. Atomic: temp file + fsync + rename next to the resolved
  target (a symlinked config stays a symlink), mode `0600`, one `<name>.bak`.
- A save refuses with `ConfigModifiedOnDisk` if the file changed since load
  (`save_force` skips the check; the TUI asks the user first).
- Unknown keys are errors (`deny_unknown_fields`). Positional sink refs (`sink = 2`) are
  accepted with a deprecation warning; the TUI always writes `desc`.
- No implicit creation: only `pwsw init` and `pwsw tui` (`load_or_init`) write a new file.
  Everything else reports `ConfigNotFound`. The daemon's reload ignores a briefly missing
  file.
- Path: `--config` > `PWSW_CONFIG` (non-empty) > `$XDG_CONFIG_HOME/pwsw/config.toml`.
  The override is stored once in a `OnceLock` (never by setting env vars), made absolute,
  and forwarded to self-spawned processes (background daemon, TUI's direct start).
- All rule regexes compile through `compile_rule_regex` (size limits), so the TUI editor,
  `test-rule` and loading accept exactly the same patterns.
- Empty sinks is a valid config (the TUI works); the daemon refuses to start without
  sinks and rejects a reload that leaves none.

**Hot reload.** The daemon watches the config's directory and, for a symlink, the
target's directory too (re-resolved after each reload). 300 ms trailing debounce, load in
`spawn_blocking`, `log_level` re-applied through the reload handle; SIGHUP reloads too.
An invalid file keeps the previous config. Config changes never need a restart, except
one: the compositor protocol is chosen once at startup (`PreferWlr` iff the config has
`output` rules, `daemon::protocol_preference`), so output rules added by a reload while on
`ext` only log a warning (`event_loop::output_rules_warning`); the connection is never
swapped.

**IPC security** (`ipc.rs`):
- Socket `$XDG_RUNTIME_DIR/pwsw.sock`; an unset, empty or relative `XDG_RUNTIME_DIR`
  falls back to `/tmp/pwsw-<uid>/pwsw.sock`, in a directory that must be a real directory
  owned by us with mode exactly `0700` (checked with `symlink_metadata`).
- The socket is never reachable by another user at any instant: bound directly in a
  private parent, else in a fresh `0700` staging dir, chmod `0600`, then hard-linked into
  place. The process umask is never changed.
- Both ends check the peer uid (`peer_cred`). Stale-socket cleanup removes only on
  `ECONNREFUSED`, and cleanup and `Drop` unlink only if the path still has the
  `(dev, ino)` that was bound.
- Length-prefixed JSON, 1 MiB message cap. Bump `PROTOCOL_VERSION` whenever a
  `Request`/`Response` changes incompatibly; a request the daemon cannot parse gets an
  error naming its version and `RESTART_HINT`.
- `Subscribe` keeps its connection open: the loop publishes a `LiveStatus` into a
  `watch` channel from `publish_status` (never blocks; at most `MAX_STATUS_SUBSCRIBERS`
  receivers), and `ipc_handler` writes a `StatusUpdate` per change (deduplicated),
  dropping a client that hangs up or does not take a write within 5 s.
- The single-instance lock is an `flock` on `pwsw.lock` beside the socket (`O_NOFOLLOW`,
  must be our regular file, never deleted). There is no PID file.

**Compositor.** Wayland runs on its own `std::thread` and must never block: it pushes
into an `EventOutbox` drained by a forwarder thread into a bounded `tokio` channel.
Coalescing per window: a newer `Changed` (or `Placement`) replaces a queued one of the
same kind *in place*; a `Placement` behind a not-yet-delivered `Opened` is folded into it (`Opened`
carries the initial placement, so a window opening on a monitor is one event); `Closed`
drops queued `Changed`/`Placement` and cancels a not-yet-delivered `Opened`. Surviving
events keep first-queued order (priority depends on it). After binding, the thread makes
`INITIAL_SYNC_ROUNDTRIPS` roundtrips and then queues `WindowEvent::InitialSync` once.
IDs are a monotonic counter, windows are keyed by `ObjectId`; closed handles are
destroyed; `Changed` is emitted only when `app_id` or title changed. `finished` or a dead
connection ends the thread with an error, so the daemon exits non-zero and systemd
restarts it. `ext` is preferred over `wlr` (`ProtocolPreference::Auto`); `PreferWlr`
picks `wlr` when available, because only `wlr` reports which monitors a window is on and
whether it is minimized: `WindowEvent::Placement` (sorted connector names from
`wl_output` v4, see `compositor/output.rs`; emitted on `done` only when changed, or at
once when an output the window is on goes away; never emitted by `ext`). Window control
requests are never used. The daemon reports its `Capabilities` (backend, monitor info) in
`Status` (`backend`, `monitor_info`) and `WindowInfo` carries `outputs`/`minimized`;
`doctor` judges availability with `ProtocolProbe::capabilities(PreferWlr)` and binds
nothing.

**TUI.** Render does no I/O: everything drawn comes from `App`. The background worker
(`tui/worker.rs`) polls daemon status every 1 s and `pw-dump` every 2 s (only while a
screen showing sinks is visible), and runs saves and daemon actions as tasks on
`detached` threads (not tokio's blocking pool, so quitting never waits for a child).
The loop sleeps until input or an update arrives and ticks only while a spinner runs.
Every command send is checked. Saves carry their snapshot back so edits made during a
save stay dirty; quitting waits (bounded) for an in-flight save. Key bindings live only in
`keymap.rs`: the help overlay is rendered from it and a test presses every documented key.
In render paths, copy small values before a `&mut` borrow instead of using `RefCell`.
Daemon control asks the running daemon how it is managed
(`GetManagerInfo`) and never trusts `INVOCATION_ID`.

**Untrusted text.** Window titles and `app_id`s are attacker-controlled. Pass them
through `sanitize::for_log` in every log call, `for_notification` for notification bodies
(markup escaped only when the server advertises `body-markup`), and `strip_ansi` for
captured child output. `get_app_icon` accepts only plain icon names.

## Conventions (enforced)

- **Toolchain** pinned in `rust-toolchain.toml` (1.100.0, also `rust-version`). Bump it
  together with both workflows, `Cargo.toml` and the README badge (checklist in
  `MAINTENANCE.md`). Edition 2024.
- **Formatting:** `cargo +nightly-2026-11-14 fmt`. `rustfmt.toml` uses nightly-only
  options; stable `cargo fmt` gives different output.
- **Lints:** clippy pedantic is enabled in the `Cargo.toml` `[lints.clippy]` table, so
  `cargo clippy --all-targets -- -D warnings` enforces it. The table allows only
  `struct_excessive_bools` and `too_many_lines`. Fix warnings instead of adding
  `#[allow]`; any new allow needs a comment saying why. `[lints.cargo]
  unused_dependencies` is on.
- **Docs** build with `RUSTDOCFLAGS="-D warnings"` (broken intra-doc links fail).
  Public functions returning `Result` document `# Errors`; anything that can panic
  documents `# Panics`. Technical terms in backticks (`PipeWire`, `app_id`, `pw-dump`).
- **Errors:** `anyhow::Result` with `Context` (`context`/`with_context`, also on
  `Option`), `anyhow::bail!`/`anyhow::anyhow!`; typed errors callers must detect
  (`ConfigNotFound`, `ConfigModifiedOnDisk`) go inside the `anyhow::Error` and are found
  with `downcast_ref`. Message texts are user-facing: the binary prints `error: {err:#}`
  (exit 1), or `Error: {err:?}` (chain + backtrace) with `RUST_BACKTRACE`/
  `RUST_LIB_BACKTRACE` set or for `daemon --foreground`. No `expect`/`unwrap` on
  production paths except documented invariants; always check a child's exit status.
- **Visibility:** private by default, `pub(crate)` across modules, `pub` only for what the
  binary or integration tests need. New struct fields private with accessors.
- **CLI output** goes through `outln!`/`errln!` and `PwswStyle` (never raw ANSI); colour
  is decided once per stream from `--color`, `NO_COLOR`, `CLICOLOR_FORCE` and the TTY.
- **Files:** atomic writes for anything important, explicit `0600`/`0700` modes; verify
  type, owner and inode before deleting.
- **User docs** live in `README.md`, `man/pwsw.1.md`, `man/pwsw.5.md`, `CHANGELOG.md`
  and the `contrib/` READMEs; update them with any behaviour change. The STATUS JSON and
  STATUS STREAM schemas in pwsw(1) are a contract: only add fields. `MAINTENANCE.md` is
  the maintainer's checklist, `SECURITY.md` the reporting policy.

## Testing rules

- **Tests never mutate the process environment** (`set_var`/`remove_var` are unsound with
  parallel test threads). Make code testable through pure functions that take the value
  (`socket_location_from`, `resolve_config_override`, `WaitPolicy::from_values`) or
  explicit paths and connections (`Config::load_from_path`, `save_to`, `spawn_compositor_thread_on`).
- **Never use the default config, runtime or data paths in tests.** Use `tempfile`
  directories. CLI smoke tests run the binary in a `Sandbox` with `env_clear()` and
  HOME/XDG pointing into a temp dir; only `LLVM_PROFILE_FILE` is passed through (for
  coverage). `sandbox_environment_is_isolated` re-runs itself with session variables set
  and checks `env` inside the sandbox.
- `bash scripts/verify_tests_safe.sh [cargo test args]` runs the suite with HOME, XDG and
  TMPDIR sandboxed and session variables removed, then fails if the real pwsw locations
  changed (config, data and state dirs, `$XDG_RUNTIME_DIR/pwsw.{sock,lock}`,
  `/tmp/pwsw-<uid>/`) or anything appeared at the sandbox's default pwsw locations
  (including `pwsw.sock` and `pwsw.lock`). It must pass.
- Coverage: `cargo llvm-cov --locked --summary-only` (needs `cargo-llvm-cov` and the
  `llvm-tools-preview` component). CI runs it as an informational job; it is not a gate.
  Review-only tests that assert nothing (`test_print_80x24_*`) are `#[ignore]`d.
- **Every wait is bounded** (receives, joins, socket reads, child processes), so a bug
  fails the test instead of hanging it.
- **No real sleeps as synchronisation.** Use sync points, channels, or tokio's paused
  clock (`test-util`); poll with a deadline when watching the file system. Fakes
  (`AudioBackend`, `SinkSwitcher`) and the mock compositor replace real services: no test
  may depend on a running PipeWire, compositor, D-Bus or daemon.
- Prefer `test-case` parameterisation (`#[test_case(a, b ; "name")]`),
  `pretty_assertions`, and `test_utils::fixtures`. Tests needing `pub(crate)` go in `src/<module>/tests/`,
  public-API tests in `tests/`.

## Workflow

- Small, reversible commits; conventional commit messages (`feat`, `fix`, `refactor`,
  `docs`, `chore`, `test`, `perf`), imperative subject, body explains why.
- **Never push without explicit user approval.** Create the commit locally, say it is not
  pushed, and wait for "push it" or similar.
- Git hooks: `bash scripts/install_git_hook.sh` (pre-commit: nightly fmt check + clippy;
  pre-push: `verify_tests_safe.sh`). `--force` replaces foreign hooks; `uninstall` and
  `print` are also available.
- Verification cycle, mandatory before pushing (the same steps as CI):

```bash
cargo +nightly-2026-11-14 fmt --check
cargo clippy --all-targets --locked -- -D warnings
bash scripts/verify_tests_safe.sh --locked
cargo check --no-default-features --locked
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --locked
```

- Manual testing: `cargo run -- daemon --foreground`, then `status`, `list-windows`,
  `test-rule`, `tui` (use `--config <tmpfile>` to keep your own config out of it).

## Decisions on record

- **CLI tools, not native PipeWire bindings.** The `pipewire` crate is unsafe FFI without
  async support, and a library fault could take the daemon down; per-action processes
  with deadlines isolate failures. Revisit only if the ecosystem changes substantially.
- **The most recently opened (or newly matching) window wins**, not the focused one.
  Focus-following could become an opt-in later, on `wlr` only (`ext` has no focus state).
- **Positional sink refs are deprecated** (they silently retarget when sinks are
  reordered by hand); still accepted, with a warning.
- **`Config` fields stay `pub`.** The only library consumer is the binary and all
  mutations go through validated methods; making them private was judged not worth the
  churn.
- **`status --json` exits 0** and reports errors inside the document (every field
  present, `null` when unknown), so status bars always get parseable output.
- **No per-app routing.** pwsw switches only the *default* sink/source; moving one
  application's stream is left to WirePlumber, which remembers a stream moved once in
  `pavucontrol` (README FAQ).
- **`anyhow`, not `color-eyre`.** Only plain `error:` lines and `Debug` reports were
  used; `color-eyre` pulled in a dozen crates for hooks pwsw did not need.
- **The status stream never carries window titles** (they change per browser tab and
  leak page titles into bars): the deciding window is `app_id`, rule `desc`/`#N` and
  rule index.
- **The compositor protocol is chosen once per daemon run**; a reload never swaps the
  Wayland connection (see Hot reload).
- **Maintenance mode** after 0.7.0: dependency/security updates and bug fixes; new
  features only if they fit the one job (README "Project status").

## Known limitations

- Compositors without `ext-foreign-toplevel-list-v1` or `wlr-foreign-toplevel-management`
  (GNOME/Mutter, KDE Plasma 6) are unsupported.
- `output` rules need `wlr-foreign-toplevel-management`; with `ext` they never match,
  and a daemon started on `ext` needs a restart once `output` rules are added.
- Profile switching works only for ACP profiles with an `output:`/`input:` mapping; UCM
  and pro-audio nodes can be used only while already active. A node name changed by
  WirePlumber rules cannot be predicted, so it is usable only while it exists.
- Default-sink/source changes made outside pwsw are not tracked; pwsw re-reads the real
  default only for manual operations (`set-sink`, `next-sink`, `prev-sink`,
  `set-source`). A fallback is upgraded only on the next window event or reload (no
  polling).
- The daemon needs at least one configured sink to start; the status stream serves at
  most 16 followers.
- The code was written by LLMs and overhauled through an orchestrated multi-agent review;
  no human Rust expert has reviewed it. Validate changes thoroughly.
