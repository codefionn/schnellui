use std::sync::OnceLock;

/// Whether the window system leaves every window without a title bar and
/// border, so a frame appears only when the client draws one.
///
/// `true` on Wayland compositors without the `xdg-decoration` protocol, such as
/// GNOME's Mutter and Weston. There the native window host falls back to a
/// frame drawn by winit; an app that wants its own title bar should open its
/// window with [`App::set_window_decorations(false)`](crate::App::set_window_decorations)
/// and draw one. `false` on Windows, macOS, X11, and Wayland compositors that
/// draw server-side decorations (KDE, wlroots, Hyprland), and when no window
/// system answers.
///
/// The first call asks the compositor once and caches the answer. On Wayland
/// that is one registry roundtrip on a short-lived connection of its own.
pub fn windows_always_undecorated() -> bool {
    static ANSWER: OnceLock<bool> = OnceLock::new();
    *ANSWER.get_or_init(super::os::windows_always_undecorated)
}
