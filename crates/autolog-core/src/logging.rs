//! The log file, for when an error message is not enough.
//!
//! Logs roll daily into the platform's data directory, and the GUI offers a
//! button to open the folder. Credentials never reach them: secrets are held
//! in [`crate::secrets::Secret`], which refuses to print itself.

use std::path::PathBuf;

use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

/// Keeps the background writer alive. Dropping this stops logging, so the
/// caller holds it for the life of the process.
#[derive(Debug)]
pub struct Logging {
    /// Where the logs are, when a file could be opened.
    pub directory: Option<PathBuf>,
    _guard: Option<tracing_appender::non_blocking::WorkerGuard>,
}

/// Start logging to a rolling file, and to stderr when run from a terminal.
///
/// `RUST_LOG` overrides the level, as usual.
pub fn init() -> Logging {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("autolog=info,warn"));

    let directory = crate::config::log_directory();
    let file = directory.as_ref().and_then(|directory| {
        std::fs::create_dir_all(directory).ok()?;
        let appender = tracing_appender::rolling::daily(directory, "autolog.log");
        Some(tracing_appender::non_blocking(appender))
    });

    let registry = tracing_subscriber::registry().with(filter).with(
        tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .with_ansi(true),
    );

    let Some((writer, guard)) = file else {
        registry.init();
        return Logging {
            directory: None,
            _guard: None,
        };
    };

    registry
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .with_ansi(false),
        )
        .init();

    Logging {
        directory,
        _guard: Some(guard),
    }
}
