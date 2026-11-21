//! Change listeners: a hook that runs after a write changed the global version.
//!
//! Hosts that cannot poll (a native view toolkit that must be told to redraw)
//! register a listener instead. The registry is a separate static, not part of
//! the runtime, so a write never touches it while the runtime lock is held.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use once_cell::sync::Lazy;
use parking_lot::Mutex;

type Listener = Arc<dyn Fn() + Send + Sync>;

/// How many listeners are registered. The write path reads only this when it
/// is zero, so a write with no listener adds one relaxed atomic load.
static LISTENER_COUNT: AtomicUsize = AtomicUsize::new(0);

static LISTENERS: Lazy<Mutex<Registry>> = Lazy::new(|| {
    Mutex::new(Registry {
        next: 0,
        entries: Vec::new(),
    })
});

struct Registry {
    next: u64,
    entries: Vec<(u64, Listener)>,
}

/// Identifies a listener for [`remove_change_listener`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChangeListenerId(u64);

/// Registers `listener` to run after every write that changed the global
/// version: `Signal::set`, `Signal::update` and a memo that recomputed to a new
/// value.
///
/// The listener runs on the writing thread, after the runtime lock is
/// released, so it may read signals, write signals or unregister itself. A
/// write made inside a listener calls the listeners again, so a listener that
/// writes must guard against its own recursion. Several listeners may be
/// registered at once. A write racing with
/// [`remove_change_listener`] may still call the removed listener once.
pub fn add_change_listener(listener: impl Fn() + Send + Sync + 'static) -> ChangeListenerId {
    let mut registry = LISTENERS.lock();
    let id = registry.next;
    registry.next += 1;
    registry.entries.push((id, Arc::new(listener)));
    LISTENER_COUNT.store(registry.entries.len(), Ordering::Release);
    ChangeListenerId(id)
}

/// Unregisters a listener. Does nothing for an id that is already removed.
pub fn remove_change_listener(id: ChangeListenerId) {
    let removed = {
        let mut registry = LISTENERS.lock();
        let position = registry.entries.iter().position(|(key, _)| *key == id.0);
        let removed = position.map(|at| registry.entries.remove(at));
        LISTENER_COUNT.store(registry.entries.len(), Ordering::Release);
        removed
    };
    // The listener drops here, outside the registry lock, because its drop may
    // unregister or read signals.
    drop(removed);
}

/// How many listeners are registered. Tests use it to see that an owner
/// unregistered its listener.
#[cfg(all(test, feature = "render-android"))]
pub(crate) fn change_listener_count() -> usize {
    LISTENER_COUNT.load(Ordering::Acquire)
}

/// Calls every listener. The caller must not hold the runtime lock.
#[inline]
pub(crate) fn notify_change() {
    if LISTENER_COUNT.load(Ordering::Acquire) == 0 {
        return;
    }
    notify_slow();
}

#[cold]
fn notify_slow() {
    let snapshot: Vec<Listener> = LISTENERS
        .lock()
        .entries
        .iter()
        .map(|(_, listener)| listener.clone())
        .collect();
    for listener in snapshot {
        listener();
    }
}
