//! Platform information used by UI hosts.
//!
//! [`PlatformPreferences`] is an immutable snapshot: it hides platform portals,
//! native accessibility calls, locale environment conventions, and their failure
//! modes behind one small interface. Missing values are intentional; policy such
//! as choosing a fallback theme or font belongs to the UI using the snapshot.
//!
//! [`watch_preferences`] reports the subset of preferences for which a target has
//! a practical change notification. [`SystemClipboard`] separately wraps the
//! stateful native clipboard and opens it only on first use. [`pick_folder`]
//! shows the native folder chooser off the UI thread.
//!
//! [`GlobalShortcuts`] registers system-wide keyboard shortcuts that fire while
//! the app's window is unfocused: the XDG portal on Linux, `RegisterHotKey` on
//! Windows, Carbon hot keys on macOS. Call [`set_app_id`] first on Linux.

mod app_id;
mod clipboard;
mod folder_picker;
mod global_shortcuts;
mod os;
mod preferences;

pub use app_id::{app_id, is_valid_app_id, portal_app_id, set_app_id, AppIdError};
pub use clipboard::{ClipboardError, ClipboardImage, SystemClipboard};
pub use folder_picker::pick_folder;
pub use global_shortcuts::{
    GlobalShortcut, GlobalShortcutBinding, GlobalShortcutError, GlobalShortcutEvent,
    GlobalShortcutPhase, GlobalShortcuts,
};
pub use preferences::{
    watch_preferences, ColorScheme, Contrast, Locale, PlatformPreferences, PreferenceChange,
};
