//! Structured diagnostics for native pointer interaction and structural remounts.
//!
//! A trace is newline-delimited JSON so it can be tailed while a window is live,
//! retained after a crash, and queried with ordinary tools such as `jq`.

use std::borrow::Cow;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;
use std::time::Instant;

use serde_json::{Map, Value};

use crate::App;

/// Environment variable understood by the built-in native host.
///
/// Set it to a file path for JSONL output, or `-`/`stderr` to stream records to
/// standard error. An explicit [`InteractionTrace`] on [`App`] takes priority.
pub const INTERACTION_TRACE_ENV: &str = "SCHNELLUI_INTERACTION_TRACE";

/// Destination and verbosity for a native interaction trace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InteractionTrace {
    pub(crate) destination: TraceDestination,
    pub(crate) include_pointer_moves: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TraceDestination {
    Stderr,
    File(PathBuf),
}

impl InteractionTrace {
    /// Writes newline-delimited JSON records to `path`, truncating an old trace.
    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self {
            destination: TraceDestination::File(path.into()),
            include_pointer_moves: true,
        }
    }

    /// Streams newline-delimited JSON records to standard error.
    pub fn stderr() -> Self {
        Self {
            destination: TraceDestination::Stderr,
            include_pointer_moves: true,
        }
    }

    /// Includes or suppresses high-volume `pointer_move` records.
    ///
    /// Button, cursor, focus, capture, warning, and remount records are always
    /// retained. Pointer moves are included by default because they are required
    /// to diagnose hit-test and cursor-boundary problems.
    pub fn include_pointer_moves(mut self, include: bool) -> Self {
        self.include_pointer_moves = include;
        self
    }

    pub(crate) fn from_env() -> Option<Self> {
        let value = std::env::var_os(INTERACTION_TRACE_ENV)?;
        let value = value.to_string_lossy();
        let value = value.trim();
        if value.is_empty() {
            return None;
        }
        Some(if matches!(value, "-" | "stderr") {
            Self::stderr()
        } else {
            Self::file(value)
        })
    }
}

/// A replacement [`App`] paired with the host-level reason that required it.
///
/// Reason strings should be stable, low-cardinality identifiers such as
/// `"route_changed"`, `"theme_changed"`, or `"browser_frame_ready"`. Dynamic
/// detail belongs in application state, not in the reason itself.
pub struct Remount {
    pub(crate) app: App,
    pub(crate) reason: Cow<'static, str>,
}

impl Remount {
    pub fn new(app: App, reason: impl Into<Cow<'static, str>>) -> Self {
        Self {
            app,
            reason: reason.into(),
        }
    }

    pub(crate) fn unspecified(app: App) -> Self {
        Self::new(app, "unspecified")
    }
}

enum TraceWriter {
    Stderr,
    File(BufWriter<File>),
}

/// One session-scoped, eagerly-flushed JSONL recorder.
pub(crate) struct InteractionRecorder {
    writer: Option<TraceWriter>,
    started: Instant,
    sequence: u64,
    include_pointer_moves: bool,
}

impl InteractionRecorder {
    pub(crate) fn open(config: Option<InteractionTrace>) -> io::Result<Option<Self>> {
        let Some(config) = config.or_else(InteractionTrace::from_env) else {
            return Ok(None);
        };
        let writer = match config.destination {
            TraceDestination::Stderr => TraceWriter::Stderr,
            TraceDestination::File(path) => TraceWriter::File(BufWriter::new(File::create(path)?)),
        };
        Ok(Some(Self {
            writer: Some(writer),
            started: Instant::now(),
            sequence: 0,
            include_pointer_moves: config.include_pointer_moves,
        }))
    }

    pub(crate) fn includes_pointer_moves(&self) -> bool {
        self.include_pointer_moves
    }

    pub(crate) fn record(&mut self, event: &'static str, payload: Value) {
        let mut object = match payload {
            Value::Object(object) => object,
            value => {
                let mut object = Map::new();
                object.insert("value".into(), value);
                object
            }
        };
        object.insert("schema".into(), Value::from("schnellui-interaction-v1"));
        object.insert("sequence".into(), Value::from(self.sequence));
        object.insert("elapsed_us".into(), Value::from(self.elapsed_us()));
        object.insert("event".into(), Value::from(event));
        self.sequence = self.sequence.saturating_add(1);

        let result = match self.writer.as_mut() {
            Some(TraceWriter::Stderr) => {
                let stderr = io::stderr();
                let mut writer = stderr.lock();
                write_record(&mut writer, &Value::Object(object))
            }
            Some(TraceWriter::File(writer)) => write_record(writer, &Value::Object(object)),
            None => return,
        };
        if let Err(error) = result {
            self.writer = None;
            eprintln!("schnellui interaction trace disabled after write failure: {error}");
        }
    }

    fn elapsed_us(&self) -> u64 {
        self.started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
    }
}

fn write_record(writer: &mut impl Write, record: &Value) -> io::Result<()> {
    serde_json::to_writer(&mut *writer, record)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_configuration_controls_pointer_move_volume() {
        let trace = InteractionTrace::file("interaction.jsonl").include_pointer_moves(false);
        assert!(!trace.include_pointer_moves);
        assert_eq!(
            trace.destination,
            TraceDestination::File(PathBuf::from("interaction.jsonl"))
        );
    }

    #[test]
    fn records_are_one_line_json_with_a_stable_schema() {
        let mut bytes = Vec::new();
        write_record(
            &mut bytes,
            &serde_json::json!({
                "schema": "schnellui-interaction-v1",
                "sequence": 3,
                "event": "remount"
            }),
        )
        .unwrap();
        assert_eq!(bytes.iter().filter(|byte| **byte == b'\n').count(), 1);
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["event"], "remount");
        assert_eq!(value["schema"], "schnellui-interaction-v1");
    }
}
