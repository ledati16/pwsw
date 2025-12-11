//! PWSW binary entry point
//!
//! Dispatches to daemon mode or subcommands based on CLI arguments.

use anyhow::Result;
use clap::Parser;
use pwsw::{cli::Args, cli::Command, commands, config::Config, daemon};

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Handle subcommands
    match args.command {
        // Daemon mode
        None | Some(Command::Daemon { .. }) => {
            let foreground = matches!(
                args.command,
                Some(Command::Daemon { foreground: true })
            );
            
            let config = Config::load()?;
            
            // Initialize logging for daemon
            let filter = tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| {
                    tracing_subscriber::EnvFilter::new(format!("pwsw={}", config.settings.log_level))
                });

            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .init();
            
            if foreground {
                tracing::info!("Running in foreground mode");
            }
            
            daemon::run(config).await
        }
        
        // IPC-based commands (require daemon)
        Some(Command::Status { json }) => {
            commands::status(json).await
        }
        
        Some(Command::Reload) => {
            commands::reload().await
        }
        
        Some(Command::Shutdown) => {
            commands::shutdown().await
        }
        
        Some(Command::ListWindows { json }) => {
            commands::list_windows(json).await
        }
        
        Some(Command::TestRule { pattern, json }) => {
            commands::test_rule(&pattern, json).await
        }
        
        // Local commands (no daemon needed)
        Some(Command::ListSinks { json }) => {
            // Initialize minimal logging for one-shot commands
            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
                )
                .init();
            
            let config = Config::load().ok();
            commands::list_sinks(config.as_ref(), json)
        }
        
        Some(Command::Validate) => {
            // Initialize minimal logging
            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
                )
                .init();
            
            let config = Config::load()?;
            config.print_summary();
            Ok(())
        }
        
        // Future feature
        Some(Command::Tui) => {
            println!("TUI not yet implemented");
            println!("The terminal user interface is planned for a future release.");
            Ok(())
        }
    }
}
