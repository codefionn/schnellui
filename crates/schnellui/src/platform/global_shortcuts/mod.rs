//! System-wide keyboard shortcuts: chords that fire while the app's window is
//! unfocused.
//!
//! An app describes the shortcuts it wants as [`GlobalShortcut`] values and
//! calls [`GlobalShortcuts::register`]. The call returns at once. Results
//! arrive as [`GlobalShortcutEvent`]s on a thread the backend owns, so the
//! callback is `Send`; [`App::register_global_shortcut`](crate::App::register_global_shortcut)
//! wraps this and runs handlers on the UI thread instead.
//!
//! Each system does it differently:
//!
//! - **Linux** asks the XDG GlobalShortcuts portal. The desktop decides the
//!   real chord and may ask the user. A preferred trigger is only a hint. The
//!   portal keys shortcuts by app id, so call [`set_app_id`](super::set_app_id)
//!   first, and install a `<app id>.desktop` file, which the portal wants
//!   before it accepts the id. Without an id the portal answers
//!   [`GlobalShortcutError::Denied`]. Pressed and released events both arrive.
//! - **Windows** calls `RegisterHotKey` on a thread that owns a message loop.
//!   The trigger is exactly the preferred one, and a shortcut without one stays
//!   unbound. The system reports no release, so there is no
//!   [`GlobalShortcutEvent::Released`]. It also reports a chord taken by
//!   another program as a [`GlobalShortcutEvent::Failed`] event.
//! - **macOS** calls Carbon `RegisterEventHotKey`. It must run on the main
//!   thread, which is where the window host runs. Media keys are not hot keys
//!   there, so such shortcuts stay unbound.
//! - Other targets, wasm included, have no backend: [`GlobalShortcuts::supported`]
//!   is `false` and registering reports [`GlobalShortcutError::Unsupported`].

use std::fmt;
use std::sync::{Arc, Mutex};

use crate::Shortcut;

mod keys;

#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod imp;
#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod imp;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
#[path = "unsupported.rs"]
mod imp;
#[cfg(target_os = "windows")]
#[path = "windows.rs"]
mod imp;

/// One shortcut an app asks the system for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalShortcut {
    id: String,
    description: String,
    trigger: Option<Shortcut>,
}

impl GlobalShortcut {
    /// A shortcut named `id`, shown to the user as `description`.
    ///
    /// The id is a stable ASCII name the app chooses, such as `toggle-window`.
    /// It may hold letters, digits, `_`, `-` and `.`. The system remembers the
    /// user's chord by id, so renaming an id loses it.
    pub fn new(id: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            description: description.into(),
            trigger: None,
        }
    }

    /// Suggests a chord. Linux treats it as a hint the desktop may ignore;
    /// Windows and macOS bind exactly this chord.
    #[must_use]
    pub fn preferred_trigger(mut self, trigger: Shortcut) -> Self {
        self.trigger = Some(trigger);
        self
    }

    /// The stable id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The text the system shows for this shortcut.
    pub fn description(&self) -> &str {
        &self.description
    }

    /// The suggested chord, if any.
    pub fn trigger(&self) -> Option<Shortcut> {
        self.trigger
    }

    /// Whether `id` is a usable id: not empty, ASCII letters, digits, `_`, `-`
    /// or `.`, and at most 128 bytes.
    pub fn is_valid_id(id: &str) -> bool {
        !id.is_empty()
            && id.len() <= 128
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
    }
}

/// What the system currently has for one registered shortcut.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalShortcutBinding {
    /// The id the app registered.
    pub id: String,
    /// The description the system holds.
    pub description: String,
    /// The chord as the system words it, such as `Ctrl+Shift+A`. Empty when
    /// the user has bound none.
    pub trigger: String,
}

/// Why global shortcuts are not working.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GlobalShortcutError {
    /// This target has no global shortcut backend.
    Unsupported,
    /// The desktop has no GlobalShortcuts portal.
    PortalMissing,
    /// The user or the system refused the request.
    Denied,
    /// The system reported something else.
    Platform(String),
}

impl fmt::Display for GlobalShortcutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => f.write_str("global shortcuts are not supported on this target"),
            Self::PortalMissing => f.write_str("the desktop has no global shortcuts portal"),
            Self::Denied => f.write_str("global shortcuts were refused"),
            Self::Platform(why) => write!(f, "global shortcuts failed: {why}"),
        }
    }
}

impl std::error::Error for GlobalShortcutError {}

/// Whether a global shortcut went down or came up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GlobalShortcutPhase {
    Pressed,
    Released,
}

/// What a backend reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GlobalShortcutEvent {
    /// The chord for this id went down.
    Pressed(String),
    /// The chord for this id came up. Windows never sends this.
    Released(String),
    /// The bindings the system holds changed, or are known for the first time.
    /// The list is complete, not a delta.
    Changed(Vec<GlobalShortcutBinding>),
    /// Something failed. Some failures, such as a chord another program holds
    /// on Windows, leave the other shortcuts working.
    Failed(GlobalShortcutError),
}

/// The channel from a backend to the app: remembers the latest bindings and
/// forwards events to the callback. Backends hold it in an `Arc`.
pub(crate) struct Sink {
    bindings: Mutex<Vec<GlobalShortcutBinding>>,
    on_event: Mutex<Box<dyn FnMut(GlobalShortcutEvent) + Send>>,
}

#[cfg_attr(
    not(any(target_os = "linux", target_os = "windows", target_os = "macos")),
    allow(dead_code)
)]
impl Sink {
    fn emit(&self, event: GlobalShortcutEvent) {
        if let Ok(mut on_event) = self.on_event.lock() {
            on_event(event);
        }
    }

    pub(crate) fn pressed(&self, id: &str) {
        self.emit(GlobalShortcutEvent::Pressed(id.to_owned()));
    }

    #[cfg_attr(target_os = "windows", allow(dead_code))] // Windows has no release event
    pub(crate) fn released(&self, id: &str) {
        self.emit(GlobalShortcutEvent::Released(id.to_owned()));
    }

    pub(crate) fn changed(&self, bindings: Vec<GlobalShortcutBinding>) {
        if let Ok(mut latest) = self.bindings.lock() {
            latest.clone_from(&bindings);
        }
        self.emit(GlobalShortcutEvent::Changed(bindings));
    }

    pub(crate) fn failed(&self, error: GlobalShortcutError) {
        self.emit(GlobalShortcutEvent::Failed(error));
    }
}

/// A live registration. Dropping it unregisters every shortcut: the portal
/// session closes, the hot keys are released, the backend thread stops.
pub struct GlobalShortcuts {
    sink: Arc<Sink>,
    _session: imp::Session,
}

impl GlobalShortcuts {
    /// Whether this target has a global shortcut backend at all. It says
    /// nothing about whether the desktop will grant one.
    pub fn supported() -> bool {
        imp::SUPPORTED
    }

    /// Registers `shortcuts` with the system and returns at once.
    ///
    /// `on_event` receives the outcome from a thread the backend owns: first
    /// [`GlobalShortcutEvent::Changed`] with what the system bound, or
    /// [`GlobalShortcutEvent::Failed`], then presses and releases. On a target
    /// without a backend it gets [`GlobalShortcutError::Unsupported`] before
    /// this call returns. On macOS the call and the drop must happen on the
    /// main thread, and `on_event` runs there.
    ///
    /// Call [`set_app_id`](crate::platform::set_app_id) first on Linux, since
    /// the portal keys shortcuts by app id.
    pub fn register(
        shortcuts: Vec<GlobalShortcut>,
        on_event: impl FnMut(GlobalShortcutEvent) + Send + 'static,
    ) -> Self {
        let sink = Arc::new(Sink {
            bindings: Mutex::new(Vec::new()),
            on_event: Mutex::new(Box::new(on_event)),
        });
        let session = imp::start(shortcuts, sink.clone());
        Self {
            sink,
            _session: session,
        }
    }

    /// The latest bindings the system reported, empty until the first
    /// [`GlobalShortcutEvent::Changed`].
    pub fn list(&self) -> Vec<GlobalShortcutBinding> {
        self.sink
            .bindings
            .lock()
            .map(|bindings| bindings.clone())
            .unwrap_or_default()
    }
}

impl fmt::Debug for GlobalShortcuts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GlobalShortcuts").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_stable_ascii_names() {
        assert!(GlobalShortcut::is_valid_id("toggle-window"));
        assert!(GlobalShortcut::is_valid_id("a.b_c-9"));
        assert!(!GlobalShortcut::is_valid_id(""));
        assert!(!GlobalShortcut::is_valid_id("with space"));
        assert!(!GlobalShortcut::is_valid_id("caf\u{e9}"));
        assert!(!GlobalShortcut::is_valid_id(&"x".repeat(129)));
    }

    #[test]
    fn builder_keeps_the_preferred_trigger() {
        let shortcut = GlobalShortcut::new("go", "Go").preferred_trigger(Shortcut::command('g'));
        assert_eq!(shortcut.id(), "go");
        assert_eq!(shortcut.description(), "Go");
        assert_eq!(shortcut.trigger(), Some(Shortcut::command('g')));
        assert_eq!(GlobalShortcut::new("x", "X").trigger(), None);
    }

    #[test]
    fn sink_remembers_the_latest_bindings() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Sink {
            bindings: Mutex::new(Vec::new()),
            on_event: Mutex::new(Box::new({
                let seen = seen.clone();
                move |event| seen.lock().unwrap().push(event)
            })),
        };
        let binding = GlobalShortcutBinding {
            id: "a".into(),
            description: "A".into(),
            trigger: "Ctrl+A".into(),
        };
        sink.changed(vec![binding.clone()]);
        sink.pressed("a");
        assert_eq!(*sink.bindings.lock().unwrap(), vec![binding.clone()]);
        assert_eq!(
            *seen.lock().unwrap(),
            vec![
                GlobalShortcutEvent::Changed(vec![binding]),
                GlobalShortcutEvent::Pressed("a".into())
            ]
        );
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    #[test]
    fn unsupported_targets_report_it_before_returning() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let handle = GlobalShortcuts::register(Vec::new(), {
            let seen = seen.clone();
            move |event| seen.lock().unwrap().push(event)
        });
        assert!(!GlobalShortcuts::supported());
        assert!(handle.list().is_empty());
        assert_eq!(
            *seen.lock().unwrap(),
            vec![GlobalShortcutEvent::Failed(
                GlobalShortcutError::Unsupported
            )]
        );
    }
}
