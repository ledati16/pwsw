//! PWSW binary entry point
//!
//! Dispatches to daemon mode or subcommands based on CLI arguments.

use clap::Parser;
use color_eyre::eyre::Result;
use pwsw::{cli::Args, cli::Command, commands, config::Config, daemon};

use std::sync::Arc;

/// Initialize logging
///
/// - For CLI commands: Use `tracing_subscriber` to log to stdout/stderr.
/// - For TUI mode: Use `TuiTracingSubscriberLayer` to capture tracing events for the widget.
fn init_logging(tui_mode: bool) {
    if tui_mode {
        // TUI mode: Use tracing layer to capture events in TUI widget
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;
        use tui_logger::TuiTracingSubscriberLayer;

        let tui_layer = TuiTracingSubscriberLayer;

        tracing_subscriber::registry().with(tui_layer).init();
    } else {
        // CLI mode: logs go to stdout/stderr based on env or default
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
            )
            .init();
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Install color-eyre for panic handling
    color_eyre::install().expect("Failed to install color_eyre");

    let args = Args::parse();

    // Handle subcommands
    match args.command {
        // No subcommand - show status or helpful message
        None => {
            init_logging(false);
            let config = Config::load()?;
            commands::status(&config, false).await
        }

        // Daemon mode
        Some(Command::Daemon { foreground }) => {
            // Daemon handles its own logging initialization (file vs stdout)
            // But we need to load config first
            let config = Config::load()?;
            daemon::run(Arc::new(config), foreground).await
        }

        // Hybrid commands (work with or without daemon)
        Some(Command::Status { json }) => {
            init_logging(false);
            let config = Config::load()?;
            commands::status(&config, json).await
        }

        // IPC-based commands (require daemon)
        Some(Command::Shutdown) => commands::shutdown().await,

        Some(Command::ListWindows { json }) => commands::list_windows(json).await,

        Some(Command::TestRule { pattern, json }) => commands::test_rule(&pattern, json).await,

        // Local commands (no daemon needed)
        Some(Command::ListSinks { json }) => {
            init_logging(false);

            let config = Config::load().ok();
            commands::list_sinks(config.as_ref(), json)
        }

        Some(Command::Validate) => {
            init_logging(false);
            let config = Config::load()?;
            config.print_summary();
            Ok(())
        }

        Some(Command::SetSink { sink }) => {
            init_logging(false);
            let config = Config::load()?;
            commands::set_sink_smart(&config, &sink)
        }

        Some(Command::NextSink) => {
            init_logging(false);
            let config = Config::load()?;
            commands::cycle_sink(&config, commands::Direction::Next)
        }

        Some(Command::PrevSink) => {
            init_logging(false);
            let config = Config::load()?;
            commands::cycle_sink(&config, commands::Direction::Prev)
        }

        // TUI - Terminal User Interface
        Some(Command::Tui) => {
            #[cfg(feature = "tui")]
            {
                init_logging(true);
                pwsw::tui::run().await
            }
            #[cfg(not(feature = "tui"))]
            {
                eprintln!("TUI feature not enabled");
                eprintln!("Rebuild with: cargo build --features tui");
                std::process::exit(1);
            }
        }
    }
}
