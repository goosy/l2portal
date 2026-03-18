// src/logger.rs — Custom env_logger setup.
//
// Log routing:
//   INFO and above  → stderr  (structured single-line, [LEVEL] module: message)
//   DEBUG and below → stderr only (same format)
//
// The format "[LEVEL] <module>: <message>" is enforced for all levels.
// Controlled by the RUST_LOG environment variable (default: info).

use env_logger::{Builder, WriteStyle};
use log::LevelFilter;
use std::io::Write;

pub fn init() {
    let mut builder = Builder::new();

    // Default level: info; overridden by RUST_LOG env var.
    builder.filter_level(LevelFilter::Info);
    builder.parse_default_env(); // respects RUST_LOG

    builder.write_style(WriteStyle::Never);

    // Format: "[LEVEL] module: message"
    // Message strings in this codebase do NOT embed a [LEVEL] prefix —
    // that is added here by the formatter.
    builder.format(|buf, record| {
        let level = record.level().as_str();
        // Use the last segment of the module path as the short tag.
        let target = record.target();
        let tag = target.rsplit("::").next().unwrap_or(target);
        writeln!(buf, "[{level}] {tag}: {}", record.args())
    });

    // All log output goes to stderr.
    builder.target(env_logger::Target::Stderr);

    builder.init();
}
