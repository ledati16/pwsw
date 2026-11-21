//! PWSW binary entry point
//!
//! Dispatches to daemon mode or subcommands based on CLI arguments.
//!
//! Exit codes: 0 success, 1 error, 2 usage error (from `clap`).

use std::process::ExitCode;
use std::sync::Arc;

use clap::{CommandFactory, Parser};
use color_eyre::eyre::{self, Result};
use pwsw::cli::{Args, Command};
use pwsw::config::Config;
use pwsw::style::{self, PwswStyle, Stream};
use pwsw::{commands, daemon};

/// Initialize logging for CLI commands.
///
/// Uses `tracing_subscriber` to log to stderr (never stdout, which may carry
/// `JSON`) based on the `RUST_LOG` env var.
fn init_logging() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .with_ansi(style::color_enabled(Stream::Stderr))
        .init();
}

/// Whether the user asked for backtraces (and so for full error reports)
fn backtrace_requested() -> bool {
    ["RUST_BACKTRACE", "RUST_LIB_BACKTRACE"]
        .iter()
        .any(|var| std::env::var_os(var).is_some_and(|v| !v.is_empty() && v != "0"))
}

/// Install `color-eyre`'s panic and error report hooks
///
/// Reports never show the source location (`Location: src/...`), and are only
/// coloured when stderr is.
fn install_error_hooks() {
    let mut builder = color_eyre::config::HookBuilder::new().display_location_section(false);
    if !style::color_enabled(Stream::Stderr) {
        // The default builders use the dark theme; an empty theme has no colours
        builder = builder.theme(color_eyre::config::Theme::new());
    }
    // Installing only fails if a hook is already set; reports then use that one
    let _ = builder.install();
}

/// Print a failed command's error to stderr
///
/// A short `error: cause: cause` line, unless backtraces were requested or the
/// daemon runs in the foreground (logs, systemd): then the full `color-eyre` report.
fn report_error(err: &eyre::Report, full_report: bool) {
    if full_report || backtrace_requested() {
        pwsw::errln!("Error: {err:?}");
    } else {
        pwsw::errln!("{} {err:#}", "error:".error().to_stderr());
    }
}

fn main() -> ExitCode {
    let args = Args::parse();
    style::init(args.color);
    install_error_hooks();

    let full_report = matches!(args.command, Some(Command::Daemon { foreground: true }));
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            let err = eyre::Report::new(e).wrap_err("Failed to start the async runtime");
            report_error(&err, false);
            return ExitCode::FAILURE;
        }
    };

    match runtime.block_on(run(args)) {
        Ok(code) => code,
        Err(err) => {
            report_error(&err, full_report);
            ExitCode::FAILURE
        }
    }
}

/// Load the config, or fail if it has no sinks (the manual switch commands need them)
fn load_config_with_sinks() -> Result<Config> {
    let config = Config::load()?;
    if config.sinks.is_empty() {
        eyre::bail!("No sinks configured. Add sinks via 'pwsw tui' first.");
    }
    Ok(config)
}

/// A command that prints its own output: success unless it failed
fn success(result: Result<()>) -> Result<ExitCode> {
    result.map(|()| ExitCode::SUCCESS)
}

/// `status` exits 1 when part of the status could not be determined (it has
/// already said why)
fn status_exit(healthy: bool) -> ExitCode {
    if healthy {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

async fn run(args: Args) -> Result<ExitCode> {
    // Register `--config` before anything resolves the config path.
    if let Some(path) = args.config.as_deref() {
        Config::set_config_path_override(path)?;
    }

    match args.command {
        // No subcommand - show status
        None => {
            init_logging();
            Ok(status_exit(commands::status(Config::load(), false).await))
        }

        // Daemon mode
        Some(Command::Daemon { foreground }) => {
            // Daemon handles its own logging initialization (file vs stdout)
            // But we need to load config first
            let config = Config::load()?;

            // Daemon requires at least one sink to operate
            if config.sinks.is_empty() {
                eyre::bail!(
                    "No sinks configured. The daemon cannot start without sinks.\n\n\
                     Add sinks via:\n  \
                     pwsw tui        (interactive setup)\n  \
                     pwsw list-sinks (discover available sinks)\n\n\
                     Or edit: {}",
                    Config::get_config_path()?.display()
                );
            }

            daemon::run(Arc::new(config), foreground).await?;
            Ok(ExitCode::SUCCESS)
        }

        // Hybrid commands (work with or without daemon)
        Some(Command::Status { json, watch: true }) => {
            init_logging();
            success(pwsw::status_watch::run(json).await)
        }
        Some(Command::Status { json, watch: false }) => {
            init_logging();
            Ok(status_exit(commands::status(Config::load(), json).await))
        }

        // IPC-based commands (require daemon)
        Some(Command::Shutdown) => success(commands::shutdown().await),

        Some(Command::ListWindows { json }) => success(commands::list_windows(json).await),

        Some(Command::TestRule { pattern, json }) => {
            success(commands::test_rule(&pattern, json).await)
        }

        // Pause / resume (require daemon)
        Some(Command::Pause) => success(commands::pause(commands::PauseAction::Pause).await),
        Some(Command::Resume) => success(commands::pause(commands::PauseAction::Resume).await),
        Some(Command::TogglePause) => success(commands::pause(commands::PauseAction::Toggle).await),

        // Local commands (no daemon needed)
        Some(Command::ListSinks { json }) => {
            init_logging();

            // The sink list is useful without a config, but say why none is shown
            let config = match Config::load() {
                Ok(config) => Some(config),
                Err(e) => {
                    pwsw::errln!(
                        "{} {}\n",
                        "⚠".warning().to_stderr(),
                        format!("Config not loaded, configured sinks not shown: {e:#}")
                            .warning()
                            .to_stderr()
                    );
                    None
                }
            };
            success(commands::list_sinks(config.as_ref(), json))
        }

        Some(Command::Validate) => {
            init_logging();
            let config = Config::load()?;

            // Warn if no sinks configured (daemon won't start)
            if config.sinks.is_empty() {
                pwsw::errln!(
                    "{} {}\n",
                    "⚠".warning().to_stderr(),
                    "No sinks configured - daemon will not start"
                        .warning()
                        .to_stderr()
                );
            }

            config.print_summary();
            Ok(ExitCode::SUCCESS)
        }

        Some(Command::Init { force }) => {
            init_logging();
            let path = Config::init(force)?;
            Config::print_init_guidance(&path);
            Ok(ExitCode::SUCCESS)
        }

        Some(Command::SetSink { sink }) => {
            init_logging();
            let config = load_config_with_sinks()?;
            success(commands::set_sink(&config, &sink).await)
        }

        Some(Command::SetSource { source }) => {
            init_logging();
            let config = Config::load()?;
            success(commands::set_source(&config, &source).await)
        }

        Some(Command::NextSink) => {
            init_logging();
            let config = load_config_with_sinks()?;
            success(commands::cycle(&config, commands::Direction::Next).await)
        }

        Some(Command::PrevSink) => {
            init_logging();
            let config = load_config_with_sinks()?;
            success(commands::cycle(&config, commands::Direction::Prev).await)
        }

        // Read-only check of the whole setup; exit 1 when a check failed
        Some(Command::Doctor) => {
            // Its own report says everything; logs only when asked for (`RUST_LOG`)
            if std::env::var_os("RUST_LOG").is_some() {
                init_logging();
            }
            Ok(status_exit(pwsw::doctor::run(args.config.is_some()).await))
        }

        Some(Command::Completions { shell }) => {
            // Generated in memory: `clap_complete` panics on a failed write (e.g. a
            // closed pipe), and `write_stdout_bytes` does not
            let mut script = Vec::new();
            clap_complete::generate(shell, &mut Args::command(), "pwsw", &mut script);
            style::write_stdout_bytes(&script);
            Ok(ExitCode::SUCCESS)
        }

        // TUI - Terminal User Interface
        Some(Command::Tui) => {
            #[cfg(feature = "tui")]
            {
                // No tracing subscriber: the TUI owns the terminal and reports
                // problems in its status line.
                pwsw::tui::run().await?;
                Ok(ExitCode::SUCCESS)
            }
            #[cfg(not(feature = "tui"))]
            {
                eyre::bail!("TUI feature not enabled; rebuild with: cargo build --features tui")
            }
        }
    }
}
