//! Console + structured file logging.
//!
//! * stderr: human-readable, coloured when attached to a terminal.
//! * `marquee.YYYY-MM-DD.log` in the log folder (see `paths`): one JSON
//!   object per line (timestamp, level, target, fields, span), rotated daily,
//!   seven files kept.

use crate::paths;
use std::io::IsTerminal;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

pub fn init(level: &str) -> Option<WorkerGuard> {
    let filter =
        || EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(format!("marquee={level},warn")));

    let console = fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .with_target(false)
        .with_filter(filter());

    let dir = paths::log_dir();
    let file = std::fs::create_dir_all(&dir).ok().and_then(|_| {
        RollingFileAppender::builder()
            .rotation(Rotation::DAILY)
            .filename_prefix("marquee")
            .filename_suffix("log")
            .max_log_files(7)
            .build(&dir)
            .ok()
    });

    let (file_layer, guard) = match file {
        Some(appender) => {
            let (writer, guard) = tracing_appender::non_blocking(appender);
            let layer = fmt::layer()
                .json()
                .with_writer(writer)
                .with_current_span(true)
                .with_span_list(false)
                .with_filter(filter());
            (Some(layer), Some(guard))
        }
        None => (None, None),
    };

    tracing_subscriber::registry().with(console).with(file_layer).init();

    std::panic::set_hook(Box::new(|info| {
        let thread = std::thread::current();
        let location = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string panic>".into());
        tracing::error!(thread = thread.name().unwrap_or("<unnamed>"), location, payload, "panic");
    }));

    guard
}
