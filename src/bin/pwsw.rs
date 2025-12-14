//! PWSW binary entry point
//!
//! Dispatches to daemon mode or subcommands based on CLI arguments.

use anyhow::Result;
use clap::Parser;
use pwsw::{cli::Args, cli::Command, commands, config::Config, daemon};

/// Initialize minimal logging for CLI commands (warn level by default)
fn init_cli_logging() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Handle subcommands
    match args.command {
        // No subcommand - show status or helpful message
        None => {
            init_cli_logging();
            let config = Config::load()?;
            commands::status(&config, false).await
        }

        // Daemon mode
        Some(Command::Daemon { foreground }) => {
            let config = Config::load()?;
            daemon::run(config, foreground).await
        }

        // Hybrid commands (work with or without daemon)
        Some(Command::Status { json }) => {
            init_cli_logging();
            let config = Config::load()?;
            commands::status(&config, json).await
        }

        // IPC-based commands (require daemon)
        Some(Command::Shutdown) => commands::shutdown().await,

        Some(Command::ListWindows { json }) => commands::list_windows(json).await,

        Some(Command::TestRule { pattern, json }) => commands::test_rule(&pattern, json).await,

        // Local commands (no daemon needed)
        Some(Command::ListSinks { json }) => {
            init_cli_logging();

            let config = Config::load().ok();
            commands::list_sinks(config.as_ref(), json)
        }

        Some(Command::Validate) => {
            init_cli_logging();
            let config = Config::load()?;
            config.print_summary();
            Ok(())
        }

        Some(Command::SetSink { sink }) => {
            init_cli_logging();
            let config = Config::load()?;
            commands::set_sink_smart(&config, &sink)
        }

        Some(Command::NextSink) => {
            init_cli_logging();
            let config = Config::load()?;
            commands::cycle_sink(&config, commands::Direction::Next)
        }

        Some(Command::PrevSink) => {
            init_cli_logging();
            let config = Config::load()?;
            commands::cycle_sink(&config, commands::Direction::Prev)
        }

        // TUI - Terminal User Interface
        Some(Command::Tui) => {
            #[cfg(feature = "tui")]
            {
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
