//! System-wide shortcuts on the app: registration, listing and dispatch.
//!
//! The registry holds what the app asked for and the handlers, which run on
//! the UI thread. The window host owns the platform session: it notices a new
//! [`revision`](GlobalShortcutRegistry::revision), starts a session for the
//! registered set and feeds backend events back through
//! [`App::apply_global_shortcut_event`]. Nothing here runs per frame.

use std::sync::atomic::{AtomicU64, Ordering};

use super::*;
use crate::platform::{
    GlobalShortcut, GlobalShortcutBinding, GlobalShortcutError, GlobalShortcutEvent,
    GlobalShortcutPhase, GlobalShortcuts,
};

type GlobalShortcutHandler = Box<dyn FnMut(GlobalShortcutPhase) + 'static>;
type GlobalShortcutObserver = Box<dyn FnMut(&[GlobalShortcutBinding]) + 'static>;

/// Hands out a new value for every change to any registry. Starting from a
/// global counter means a registry built after a remount never matches the
/// revision the host last synced.
static NEXT_REVISION: AtomicU64 = AtomicU64::new(1);

#[derive(Default)]
pub(crate) struct GlobalShortcutRegistry {
    entries: Vec<(GlobalShortcut, GlobalShortcutHandler)>,
    bindings: Vec<GlobalShortcutBinding>,
    error: Option<GlobalShortcutError>,
    observer: Option<GlobalShortcutObserver>,
    /// Changes whenever the registered set does. 0 for a registry never used.
    revision: u64,
    /// The revision the platform session was started for.
    synced: u64,
    /// The platform session, once the window host started one.
    session: Option<GlobalShortcuts>,
}

impl GlobalShortcutRegistry {
    fn touch(&mut self) {
        self.revision = NEXT_REVISION.fetch_add(1, Ordering::Relaxed);
    }

    fn notify(&mut self) {
        if let Some(observer) = &mut self.observer {
            observer(&self.bindings);
        }
    }
}

impl App {
    /// Registers a system-wide shortcut and the handler to run when it fires,
    /// replacing an earlier registration with the same id. Returns `false`
    /// and registers nothing when the id is not valid (see
    /// [`GlobalShortcut::is_valid_id`]).
    ///
    /// The handler runs on the UI thread with [`GlobalShortcutPhase::Pressed`]
    /// when the chord goes down and [`GlobalShortcutPhase::Released`] when it
    /// comes up (Windows never sends the release). It fires while the app's
    /// window is unfocused. Only the native window host asks the system for
    /// shortcuts; headless runs and tests never do, and fire handlers with
    /// [`App::dispatch_global_shortcut`].
    ///
    /// On Linux the portal keys shortcuts by app id, so call
    /// [`platform::set_app_id`] before the app
    /// runs. The desktop decides the real chord; see [`App::global_shortcuts`]
    /// for what it chose.
    pub fn register_global_shortcut(
        &mut self,
        shortcut: GlobalShortcut,
        handler: impl FnMut(GlobalShortcutPhase) + 'static,
    ) -> bool {
        if !GlobalShortcut::is_valid_id(shortcut.id()) {
            return false;
        }
        let registry = &mut self.global_shortcuts;
        let handler: GlobalShortcutHandler = Box::new(handler);
        match registry
            .entries
            .iter_mut()
            .find(|(known, _)| known.id() == shortcut.id())
        {
            Some(entry) => *entry = (shortcut, handler),
            None => registry.entries.push((shortcut, handler)),
        }
        registry.touch();
        true
    }

    /// Removes the global shortcut with this id. Returns whether there was one.
    pub fn unregister_global_shortcut(&mut self, id: &str) -> bool {
        let registry = &mut self.global_shortcuts;
        let before = registry.entries.len();
        registry.entries.retain(|(known, _)| known.id() != id);
        let removed = registry.entries.len() != before;
        if removed {
            registry.touch();
        }
        removed
    }

    /// The global shortcuts the app registered, in registration order.
    pub fn registered_global_shortcuts(&self) -> impl Iterator<Item = &GlobalShortcut> {
        self.global_shortcuts
            .entries
            .iter()
            .map(|(shortcut, _)| shortcut)
    }

    /// What the system currently has bound for the app's global shortcuts.
    /// Empty until the platform reports, and always empty in headless runs.
    /// An entry with an empty trigger is one the user has bound to no chord.
    pub fn global_shortcuts(&self) -> &[GlobalShortcutBinding] {
        &self.global_shortcuts.bindings
    }

    /// Why global shortcuts are not working, if the platform said so.
    pub fn global_shortcut_error(&self) -> Option<&GlobalShortcutError> {
        self.global_shortcuts.error.as_ref()
    }

    /// Runs `observer` on the UI thread whenever the system's bindings
    /// change, so a signal can show the current chords. Replaces an earlier
    /// observer.
    pub fn on_global_shortcuts_changed(
        &mut self,
        observer: impl FnMut(&[GlobalShortcutBinding]) + 'static,
    ) {
        self.global_shortcuts.observer = Some(Box::new(observer));
    }

    /// Fires the handler registered under `id`, as the platform would. Returns
    /// `true` when one was registered. Headless tests and the debug server
    /// use this to drive a shortcut without a desktop.
    pub fn dispatch_global_shortcut(&mut self, id: &str, phase: GlobalShortcutPhase) -> bool {
        let Some((_, handler)) = self
            .global_shortcuts
            .entries
            .iter_mut()
            .find(|(known, _)| known.id() == id)
        else {
            return false;
        };
        handler(phase);
        true
    }

    /// Applies one backend event on the UI thread. Returns whether anything
    /// observable ran, so the host knows to redraw.
    pub(crate) fn apply_global_shortcut_event(&mut self, event: GlobalShortcutEvent) -> bool {
        match event {
            GlobalShortcutEvent::Pressed(id) => {
                self.dispatch_global_shortcut(&id, GlobalShortcutPhase::Pressed)
            }
            GlobalShortcutEvent::Released(id) => {
                self.dispatch_global_shortcut(&id, GlobalShortcutPhase::Released)
            }
            GlobalShortcutEvent::Changed(bindings) => {
                let registry = &mut self.global_shortcuts;
                registry.error = None;
                registry.bindings = bindings;
                registry.notify();
                true
            }
            GlobalShortcutEvent::Failed(error) => {
                let registry = &mut self.global_shortcuts;
                registry.error = Some(error);
                registry.notify();
                true
            }
        }
    }

    /// Whether the registered set changed since the host last started a
    /// platform session. One integer comparison.
    pub(crate) fn global_shortcuts_stale(&self) -> bool {
        self.global_shortcuts.revision != self.global_shortcuts.synced
    }

    /// Replaces the platform session with one for the registered set, or ends
    /// it when nothing is registered. `start` gets the set and returns the new
    /// session.
    pub(crate) fn sync_global_shortcut_session(
        &mut self,
        start: impl FnOnce(Vec<GlobalShortcut>) -> GlobalShortcuts,
    ) {
        let registry = &mut self.global_shortcuts;
        registry.synced = registry.revision;
        // Closing the old session first lets the portal see one session.
        registry.session = None;
        if registry.entries.is_empty() {
            registry.bindings.clear();
            registry.error = None;
            registry.notify();
            return;
        }
        let set = registry.entries.iter().map(|(s, _)| s.clone()).collect();
        registry.session = Some(start(set));
    }

    /// Moves the registrations, bindings and session into a rebuilt app.
    pub(crate) fn inherit_global_shortcuts(&mut self, previous: &mut App) {
        self.global_shortcuts = std::mem::take(&mut previous.global_shortcuts);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn app() -> App {
        App::new(64, 64)
    }

    fn shortcut(id: &str) -> GlobalShortcut {
        GlobalShortcut::new(id, id)
    }

    #[test]
    fn dispatch_runs_the_handler_registered_under_the_id() {
        let mut app = app();
        let seen = Rc::new(RefCell::new(Vec::new()));
        assert!(app.register_global_shortcut(shortcut("a"), {
            let seen = seen.clone();
            move |phase| seen.borrow_mut().push(("a", phase))
        }));
        assert!(app.register_global_shortcut(shortcut("b"), {
            let seen = seen.clone();
            move |phase| seen.borrow_mut().push(("b", phase))
        }));
        assert!(app.dispatch_global_shortcut("b", GlobalShortcutPhase::Pressed));
        assert!(app.dispatch_global_shortcut("a", GlobalShortcutPhase::Released));
        assert!(!app.dispatch_global_shortcut("missing", GlobalShortcutPhase::Pressed));
        assert_eq!(
            *seen.borrow(),
            vec![
                ("b", GlobalShortcutPhase::Pressed),
                ("a", GlobalShortcutPhase::Released)
            ]
        );
    }

    #[test]
    fn registering_the_same_id_replaces_the_handler_and_keeps_its_place() {
        let mut app = app();
        let count = Rc::new(RefCell::new((0, 0)));
        app.register_global_shortcut(shortcut("a"), {
            let count = count.clone();
            move |_| count.borrow_mut().0 += 1
        });
        app.register_global_shortcut(shortcut("b"), |_| {});
        app.register_global_shortcut(
            GlobalShortcut::new("a", "second").preferred_trigger(Shortcut::command('j')),
            {
                let count = count.clone();
                move |_| count.borrow_mut().1 += 1
            },
        );
        app.dispatch_global_shortcut("a", GlobalShortcutPhase::Pressed);
        assert_eq!(*count.borrow(), (0, 1));
        let ids: Vec<_> = app
            .registered_global_shortcuts()
            .map(|s| (s.id(), s.description()))
            .collect();
        assert_eq!(ids, vec![("a", "second"), ("b", "b")]);
    }

    #[test]
    fn unregister_removes_and_reports() {
        let mut app = app();
        app.register_global_shortcut(shortcut("a"), |_| {});
        assert!(app.unregister_global_shortcut("a"));
        assert!(!app.unregister_global_shortcut("a"));
        assert!(!app.dispatch_global_shortcut("a", GlobalShortcutPhase::Pressed));
        assert_eq!(app.registered_global_shortcuts().count(), 0);
    }

    #[test]
    fn invalid_ids_are_refused() {
        let mut app = app();
        assert!(!app.register_global_shortcut(shortcut("has space"), |_| {}));
        assert!(!app.register_global_shortcut(shortcut(""), |_| {}));
        assert_eq!(app.registered_global_shortcuts().count(), 0);
        assert!(!app.global_shortcuts_stale());
    }

    #[test]
    fn the_set_going_stale_is_what_restarts_the_session() {
        let mut app = app();
        assert!(!app.global_shortcuts_stale());
        app.register_global_shortcut(shortcut("a"), |_| {});
        assert!(app.global_shortcuts_stale());
        // A dispatch is not a change to the set.
        app.sync_global_shortcut_session(|set| {
            assert_eq!(set.len(), 1);
            GlobalShortcuts::register(set, |_| {})
        });
        assert!(!app.global_shortcuts_stale());
        app.dispatch_global_shortcut("a", GlobalShortcutPhase::Pressed);
        assert!(!app.global_shortcuts_stale());
        app.unregister_global_shortcut("a");
        assert!(app.global_shortcuts_stale());
        app.sync_global_shortcut_session(|_| unreachable!("nothing left to register"));
        assert!(!app.global_shortcuts_stale());
    }

    #[test]
    fn backend_events_reach_handlers_the_list_and_the_observer() {
        let mut app = app();
        let pressed = Rc::new(RefCell::new(0));
        app.register_global_shortcut(shortcut("a"), {
            let pressed = pressed.clone();
            move |phase| {
                if phase == GlobalShortcutPhase::Pressed {
                    *pressed.borrow_mut() += 1;
                }
            }
        });
        let observed = Rc::new(RefCell::new(Vec::new()));
        app.on_global_shortcuts_changed({
            let observed = observed.clone();
            move |bindings| {
                observed.borrow_mut().push(
                    bindings
                        .iter()
                        .map(|b| b.trigger.clone())
                        .collect::<Vec<_>>(),
                )
            }
        });
        let binding = GlobalShortcutBinding {
            id: "a".into(),
            description: "a".into(),
            trigger: "Ctrl+A".into(),
        };
        assert!(
            app.apply_global_shortcut_event(GlobalShortcutEvent::Changed(vec![binding.clone()]))
        );
        assert_eq!(app.global_shortcuts(), &[binding]);
        assert!(app.apply_global_shortcut_event(GlobalShortcutEvent::Pressed("a".into())));
        assert!(!app.apply_global_shortcut_event(GlobalShortcutEvent::Pressed("zzz".into())));
        assert_eq!(*pressed.borrow(), 1);
        assert!(app
            .apply_global_shortcut_event(GlobalShortcutEvent::Failed(GlobalShortcutError::Denied)));
        assert_eq!(
            app.global_shortcut_error(),
            Some(&GlobalShortcutError::Denied)
        );
        assert!(app.apply_global_shortcut_event(GlobalShortcutEvent::Changed(Vec::new())));
        assert_eq!(app.global_shortcut_error(), None);
        assert_eq!(
            *observed.borrow(),
            vec![vec!["Ctrl+A".to_owned()], vec!["Ctrl+A".to_owned()], vec![]]
        );
    }

    #[test]
    fn a_rebuilt_app_inherits_the_registry() {
        let mut old = app();
        old.register_global_shortcut(shortcut("a"), |_| {});
        old.apply_global_shortcut_event(GlobalShortcutEvent::Changed(vec![
            GlobalShortcutBinding {
                id: "a".into(),
                description: "a".into(),
                trigger: "F5".into(),
            },
        ]));
        let revision = old.global_shortcuts.revision;
        let mut new = app();
        new.inherit_global_shortcuts(&mut old);
        assert!(new.dispatch_global_shortcut("a", GlobalShortcutPhase::Pressed));
        assert_eq!(new.global_shortcuts().len(), 1);
        assert_eq!(new.global_shortcuts.revision, revision);
        assert!(!old.dispatch_global_shortcut("a", GlobalShortcutPhase::Pressed));
    }

    #[test]
    fn in_app_shortcuts_accept_keys_without_a_character() {
        let mut app = app();
        let hits = Rc::new(RefCell::new(0));
        app.register_shortcut(Shortcut::key(ShortcutKey::F(5), false, false, false), {
            let hits = hits.clone();
            move || *hits.borrow_mut() += 1
        });
        assert!(app.dispatch_shortcut(Shortcut::key(ShortcutKey::F(5), false, false, false)));
        assert!(!app.dispatch_shortcut(Shortcut::key(ShortcutKey::F(5), true, false, false)));
        assert!(!app.dispatch_shortcut(Shortcut::new('5', false, false, false)));
        assert_eq!(*hits.borrow(), 1);
        // Existing character shortcuts still match case-insensitively.
        app.register_shortcut(Shortcut::command('K'), || {});
        assert!(app.dispatch_shortcut(Shortcut::command('k')));
    }
}
