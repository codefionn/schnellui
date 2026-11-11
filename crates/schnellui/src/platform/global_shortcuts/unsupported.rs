//! No backend: reports [`GlobalShortcutError::Unsupported`] and holds nothing.

use std::sync::Arc;

use super::{GlobalShortcut, GlobalShortcutError, Sink};

pub(super) const SUPPORTED: bool = false;

pub(super) struct Session;

pub(super) fn start(_shortcuts: Vec<GlobalShortcut>, sink: Arc<Sink>) -> Session {
    sink.failed(GlobalShortcutError::Unsupported);
    Session
}
