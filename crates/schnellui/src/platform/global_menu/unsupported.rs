//! Targets with no global menu: Windows, Linux for now, wasm and the rest.

use std::sync::Arc;

use super::{MenuBar, Sink};

pub(super) const SUPPORTED: bool = false;

pub(super) struct Session;

pub(super) fn start(_menu: &MenuBar, _sink: Arc<Sink>) -> Session {
    Session
}
