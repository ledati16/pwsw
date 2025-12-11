//! PWSW binary entry point
//!
//! Dispatches to daemon mode or one-shot commands based on CLI arguments.

use anyhow::Result;
use clap::Parser;
use pwsw::{cli::Args, commands, config::Config, daemon};

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Determine if this is a one-shot command
    let is_oneshot = args.list_sinks
        || args.set_sink.is_some()
        || args.get_sink
        || args.next_sink
        || args.prev_sink
        || args.check_config;

    // Initialize logging for one-shot commands only
    // Daemon mode inits after loading config to respect log_level setting
    if is_oneshot {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
            )
            .init();
    }

    // One-shot commands
    if args.list_sinks {
        let config = Config::load().ok();
        return commands::list_sinks(config.as_ref(), args.json);
    }

    if args.get_sink {
        let config = Config::load()?;
        return commands::get_current_sink(&config, args.json);
    }

    let config = Config::load()?;

    if args.check_config {
        config.print_summary();
        return Ok(());
    }

    if let Some(ref sink_ref) = args.set_sink {
        return commands::set_sink_smart(&config, sink_ref);
    }

    if args.next_sink {
        return commands::cycle_sink(&config, commands::Direction::Next);
    }

    if args.prev_sink {
        return commands::cycle_sink(&config, commands::Direction::Prev);
    }

    // Daemon mode
    daemon::run(config).await
}
