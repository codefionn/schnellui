//! The XDG GlobalShortcuts portal, on a thread of its own.
//!
//! The thread creates a session, binds the shortcuts, then forwards the
//! portal's `Activated`, `Deactivated` and `ShortcutsChanged` signals until
//! the handle drops, which closes the session.

use std::future::poll_fn;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::Poll;

use ashpd::desktop::global_shortcuts::{GlobalShortcuts as Portal, NewShortcut, Shortcut};
use ashpd::desktop::ResponseError;
use ashpd::{Error, PortalError};
use futures_util::task::AtomicWaker;
use futures_util::{stream, StreamExt};

use super::keys::xdg_trigger;
use super::{GlobalShortcut, GlobalShortcutBinding, GlobalShortcutError, Sink};

pub(super) const SUPPORTED: bool = true;

/// Tells the portal thread to close its session and end.
#[derive(Default)]
struct Stop {
    flag: AtomicBool,
    waker: AtomicWaker,
}

impl Stop {
    async fn requested(&self) {
        poll_fn(|cx| {
            if self.flag.load(Ordering::Acquire) {
                return Poll::Ready(());
            }
            self.waker.register(cx.waker());
            if self.flag.load(Ordering::Acquire) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await
    }
}

pub(super) struct Session {
    stop: Arc<Stop>,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop.flag.store(true, Ordering::Release);
        self.stop.waker.wake();
    }
}

pub(super) fn start(shortcuts: Vec<GlobalShortcut>, sink: Arc<Sink>) -> Session {
    let stop = Arc::new(Stop::default());
    let thread_stop = stop.clone();
    let spawned = std::thread::Builder::new()
        .name("schnellui-global-shortcuts".into())
        .spawn({
            let sink = sink.clone();
            move || {
                if let Err(error) = async_io::block_on(run(shortcuts, &sink, &thread_stop)) {
                    sink.failed(error);
                }
            }
        });
    if let Err(error) = spawned {
        sink.failed(GlobalShortcutError::Platform(error.to_string()));
    }
    Session { stop }
}

enum Signal {
    Activated(String),
    Deactivated(String),
    Changed,
    Stop,
}

async fn run(
    shortcuts: Vec<GlobalShortcut>,
    sink: &Sink,
    stop: &Stop,
) -> Result<(), GlobalShortcutError> {
    let portal = Portal::new().await.map_err(failure)?;
    // Subscribe before binding, so a press right after the bind is not lost.
    let activated = portal.receive_activated().await.map_err(failure)?;
    let deactivated = portal.receive_deactivated().await.map_err(failure)?;
    let changed = portal.receive_shortcuts_changed().await.map_err(failure)?;
    let session = portal
        .create_session(Default::default())
        .await
        .map_err(failure)?;
    let wanted: Vec<NewShortcut> = shortcuts
        .iter()
        .map(|shortcut| {
            let trigger = shortcut.trigger().and_then(xdg_trigger);
            NewShortcut::new(shortcut.id(), shortcut.description())
                .preferred_trigger(trigger.as_deref())
        })
        .collect();
    let bound = portal
        .bind_shortcuts(&session, &wanted, None, Default::default())
        .await
        .map_err(failure)?
        .response()
        .map_err(failure)?;
    sink.changed(bindings(bound.shortcuts()));

    let events = stream::select(
        stream::select(
            activated.map(|event| Signal::Activated(event.shortcut_id().to_owned())),
            deactivated.map(|event| Signal::Deactivated(event.shortcut_id().to_owned())),
        ),
        stream::select(
            changed.map(|_| Signal::Changed),
            stream::once(stop.requested()).map(|()| Signal::Stop),
        ),
    );
    let mut events = std::pin::pin!(events);
    while let Some(event) = events.next().await {
        match event {
            Signal::Activated(id) => sink.pressed(&id),
            Signal::Deactivated(id) => sink.released(&id),
            Signal::Changed => {
                // The signal may carry only what changed; the list is the whole.
                let listed = portal
                    .list_shortcuts(&session, Default::default())
                    .await
                    .map_err(failure)?
                    .response()
                    .map_err(failure)?;
                sink.changed(bindings(listed.shortcuts()));
            }
            Signal::Stop => break,
        }
    }
    let _ = session.close().await;
    Ok(())
}

fn bindings(shortcuts: &[Shortcut]) -> Vec<GlobalShortcutBinding> {
    shortcuts
        .iter()
        .map(|shortcut| GlobalShortcutBinding {
            id: shortcut.id().to_owned(),
            description: shortcut.description().to_owned(),
            trigger: shortcut.trigger_description().to_owned(),
        })
        .collect()
}

fn failure(error: Error) -> GlobalShortcutError {
    match error {
        Error::PortalNotFound(_) => GlobalShortcutError::PortalMissing,
        Error::Response(ResponseError::Cancelled) => GlobalShortcutError::Denied,
        Error::Portal(PortalError::NotAllowed(_) | PortalError::Cancelled(_)) => {
            GlobalShortcutError::Denied
        }
        other => GlobalShortcutError::Platform(other.to_string()),
    }
}
