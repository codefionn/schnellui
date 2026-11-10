//! A windowed host with no window, for property tests that need the real
//! pointer and key paths: press capture, slider scrubbing, text drags, dialog
//! moves, context menus and the remount poll all run exactly as they do on a
//! desktop. Exposed as [`crate::testing::fuzz::HeadlessHost`].

use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton};
use winit::keyboard::ModifiersState;

use super::{HostClipboard, WindowedApp};
use crate::debug_server::DebugKey;
use crate::scene::{Point, Rect, Size};
use crate::widgets::Theme;
use crate::{App, WindowUpdate};

/// A pointer button the headless host can press.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PointerButton {
    Left,
    Right,
}

/// A clipboard command, as Ctrl+C, Ctrl+X and Ctrl+V give the window host.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ClipboardAction {
    Copy,
    Cut,
    Paste,
}

/// A key the headless host can deliver, named as the debug server names them.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum HostKey {
    Tab,
    Enter,
    Space,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Escape,
    SelectAll,
    /// Typed text, delivered as one character event.
    Text(String),
}

impl HostKey {
    fn name(&self) -> &'static str {
        match self {
            HostKey::Tab => "tab",
            HostKey::Enter => "enter",
            HostKey::Space => "space",
            HostKey::Backspace => "backspace",
            HostKey::Delete => "delete",
            HostKey::Left => "left",
            HostKey::Right => "right",
            HostKey::Up => "up",
            HostKey::Down => "down",
            HostKey::Home => "home",
            HostKey::End => "end",
            HostKey::PageUp => "page_up",
            HostKey::PageDown => "page_down",
            HostKey::Escape => "escape",
            HostKey::SelectAll => "select_all",
            HostKey::Text(_) => "text",
        }
    }
}

/// Drives a mounted [`App`] through the windowed host's input handlers
/// without opening a window or a GPU surface. Every input runs the host's
/// remount poll and redraw, so each call ends on a settled frame.
pub struct HeadlessHost {
    host: WindowedApp,
    /// The theme last set, handed to the remount hook so a rebuilt app
    /// keeps it, as an application that owns its theme does.
    theme: Rc<Cell<Theme>>,
    /// Which of left and right is held. A real pointer never presses a held
    /// button again or releases one that is up.
    held: [bool; 2],
    /// Virtual time. The app's clock is pinned to it before every frame, so
    /// animations and theme transitions advance only through
    /// [`HeadlessHost::advance`].
    now: Instant,
    /// While set, input is delivered without a redraw after each event, as
    /// events that arrive faster than the display refreshes are.
    batching: bool,
}

impl HeadlessHost {
    /// Takes over an app that has already run its first frame.
    pub fn new(app: App) -> HeadlessHost {
        let size = app.size();
        let scale = app.scale();
        let glyph_atlas_id = app.atlas.id();
        let theme = Rc::new(Cell::new(app.theme()));
        HeadlessHost {
            theme,
            held: [false; 2],
            now: Instant::now(),
            batching: false,
            host: WindowedApp {
                app,
                clipboard: HostClipboard::Memory(None),
                headless: true,
                title: "headless".into(),
                window: None,
                renderer: None,
                accessibility: None,
                event_loop_proxy: None,
                accessibility_full_update_pending: true,
                accessibility_published: Default::default(),
                cursor: PhysicalPosition::new(-1000.0, -1000.0),
                cursor_inside: false,
                cursor_icon: Default::default(),
                cursor_synced_key: None,
                ime_area: None,
                glyph_atlas_id,
                interaction_trace: None,
                _debug_server: None,
                remount_trigger: "startup",
                remount_count: 0,
                subtree_replacement_count: 0,
                remount_counts_by_reason: BTreeMap::new(),
                last_remount: None,
                init_phys: PhysicalSize::new(
                    (size.width * scale) as u32,
                    (size.height * scale) as u32,
                ),
                deadline: None,
                interval_deadline: None,
                animation_deadline: None,
                modifiers: ModifiersState::default(),
                control_pressed: false,
                raw_pointer_capture: Vec::new(),
                drag_text: None,
                last_text_click: None,
                drag_slider: None,
                left_pointer_down: false,
                remount: None,
            },
        }
    }

    /// Installs the application's structural update hook, polled after
    /// every input as the windowed host polls it. The hook gets the viewport
    /// and the theme last set through [`HeadlessHost::set_theme`].
    pub fn set_remount(
        &mut self,
        mut hook: impl FnMut(Size, Theme) -> Option<WindowUpdate> + 'static,
    ) {
        let theme = self.theme.clone();
        self.host.remount = Some(Box::new(move |size| hook(size, theme.get())));
    }

    /// Accepted structural remounts and subtree replacements so far.
    pub fn remounts(&self) -> u64 {
        self.host.remount_count + self.host.subtree_replacement_count
    }

    pub fn app(&self) -> &App {
        &self.host.app
    }

    pub fn app_mut(&mut self) -> &mut App {
        &mut self.host.app
    }

    /// The pointer position in logical pixels, or `None` once it left.
    pub fn pointer(&self) -> Option<Point> {
        self.host.cursor_inside.then(|| self.host.cursor_logical())
    }

    /// Pins the app's clock to virtual time. A remount brings a new app
    /// with an unpinned clock, so this runs before every poll and frame.
    fn pin_clock(&mut self) {
        let now = self.now;
        self.host.app.set_clock(Some(now));
    }

    fn settle(&mut self) {
        self.pin_clock();
        self.host.poll_remount();
        if !self.batching {
            self.pin_clock();
            self.host.on_redraw_requested();
        }
        self.host.sync_ime();
    }

    /// With batching on, input events are handled without a redraw between
    /// them, as a burst that arrives within one display frame is. Turning it
    /// off, or calling [`HeadlessHost::frame`], draws the frame.
    pub fn set_batching(&mut self, batching: bool) {
        self.batching = batching;
        if !batching {
            self.frame();
        }
    }

    /// Lets `duration` of virtual time pass, then draws a frame.
    pub fn advance(&mut self, duration: Duration) {
        self.now += duration;
        self.pin_clock();
        self.host.poll_remount();
        self.pin_clock();
        self.host.on_redraw_requested();
    }

    /// Whether the app still wants animation frames: a theme transition, a
    /// floating label, a spinner or edge auto-scroll is running.
    pub fn animating(&self) -> bool {
        self.host.app.wants_animation_frame()
    }

    /// Whether input is being batched with no frame drawn between events.
    pub fn batching(&self) -> bool {
        self.batching
    }

    /// Whether a pointer button is down.
    pub fn held(&self) -> bool {
        self.held.iter().any(|&down| down)
    }

    /// Whether an animated theme change is still in flight. Each of its
    /// frames rebuilds the tree, so until it ends the app has not yet been
    /// rebuilt under the theme it is heading for.
    pub fn theme_fading(&self) -> bool {
        self.host.app.theme_transition_active()
    }

    /// Advances virtual time one display frame (`step`) at a time until no
    /// animation is running or `limit` has passed, calling `each` after every
    /// frame. Returns the time it took.
    pub fn run_animations(
        &mut self,
        step: Duration,
        limit: Duration,
        mut each: impl FnMut(&mut HeadlessHost),
    ) -> Duration {
        let mut spent = Duration::ZERO;
        while self.animating() && spent < limit {
            self.advance(step);
            spent += step;
            each(self);
        }
        spent
    }

    /// Starts an animated theme change over `duration` of virtual time.
    pub fn transition_theme(&mut self, theme: Theme, duration: Duration) {
        self.theme.set(theme);
        self.pin_clock();
        if self.host.app.transition_theme(theme, duration) {
            // What the window host does after a platform theme change: an
            // instant change has rebuilt the tree, a fade has only started.
            self.host.sync_renderer_after_theme_rebuild();
        }
        self.settle();
    }

    /// Moves the pointer to a logical point. Points outside the window are
    /// clamped to its edge, since a real pointer never reports one.
    pub fn pointer_move(&mut self, p: Point) {
        let size = self.host.app.size();
        let scale = f64::from(self.host.app.scale());
        let x = p.x.clamp(0.0, (size.width - 1.0).max(0.0));
        let y = p.y.clamp(0.0, (size.height - 1.0).max(0.0));
        self.host.on_cursor_moved(PhysicalPosition::new(
            f64::from(x) * scale,
            f64::from(y) * scale,
        ));
        self.settle();
    }

    /// The pointer leaves the window.
    pub fn pointer_leave(&mut self) {
        let _ = self.host.on_cursor_left();
        self.settle();
    }

    /// Presses a button. Pressing one that is already held does nothing.
    pub fn press(&mut self, button: PointerButton) {
        if std::mem::replace(&mut self.held[button as usize], true) {
            return;
        }
        self.host
            .on_mouse_input(ElementState::Pressed, winit_button(button));
        self.settle();
    }

    /// Releases a button. Releasing one that is up does nothing.
    pub fn release(&mut self, button: PointerButton) {
        if !std::mem::replace(&mut self.held[button as usize], false) {
            return;
        }
        self.host
            .on_mouse_input(ElementState::Released, winit_button(button));
        self.settle();
    }

    /// Delivers one input-method event, as the window's `Ime` event does.
    pub fn ime(&mut self, event: crate::RawImeEvent) {
        let ime = match event {
            crate::RawImeEvent::Enabled => winit::event::Ime::Enabled,
            crate::RawImeEvent::Disabled => winit::event::Ime::Disabled,
            crate::RawImeEvent::Preedit { text, cursor } => {
                winit::event::Ime::Preedit(text, cursor)
            }
            crate::RawImeEvent::Commit(text) => winit::event::Ime::Commit(text),
        };
        let _ = self.host.on_ime(ime);
        self.settle();
    }

    /// The logical area handed to the input method, `None` while it is off.
    pub fn ime_area(&self) -> Option<Rect> {
        self.host.ime_area
    }

    /// Delivers one key, as the debug server's `key` command does.
    pub fn key(&mut self, key: &HostKey, shift: bool, ctrl: bool) {
        let text = match key {
            HostKey::Text(text) => Some(text.clone()),
            _ => None,
        };
        let _ = self.host.handle_debug_key(DebugKey {
            key: key.name().into(),
            shift,
            ctrl,
            text,
        });
        self.settle();
    }

    /// One wheel notch at the pointer, positive scrolling down.
    pub fn wheel(&mut self, delta_y: f32) {
        // A window only gets wheel events while the pointer is over it.
        let Some(p) = self.pointer() else {
            return;
        };
        self.host.app.dispatch_wheel_at(p, delta_y);
        self.settle();
    }

    /// Resizes the window to a logical size. A window that shrinks out from
    /// under the pointer loses it, as a compositor reports.
    pub fn resize(&mut self, width: f32, height: f32) {
        self.host.app.resize(width, height);
        self.settle();
        if self
            .pointer()
            .is_some_and(|p| p.x >= width || p.y >= height)
        {
            self.pointer_leave();
        }
    }

    /// Copies, cuts or pastes through the window host's own clipboard
    /// commands, against an in-memory clipboard.
    pub fn clipboard(&mut self, action: ClipboardAction) {
        let _ = match action {
            ClipboardAction::Copy => self.host.copy_or_cut(false),
            ClipboardAction::Cut => self.host.copy_or_cut(true),
            ClipboardAction::Paste => self.host.paste(),
        };
        self.settle();
    }

    /// Clears keyboard focus.
    pub fn blur(&mut self) {
        self.host.app.focus(None);
        self.settle();
    }

    /// Switches the theme, then lets hover catch up with the resting pointer.
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme.set(theme);
        if self.host.app.set_theme(theme) {
            self.host.sync_renderer_after_theme_rebuild();
        }
        self.settle();
    }

    /// Draws a frame with no input, even while batching.
    pub fn frame(&mut self) {
        self.pin_clock();
        self.host.poll_remount();
        self.pin_clock();
        self.host.on_redraw_requested();
    }
}

fn winit_button(button: PointerButton) -> MouseButton {
    match button {
        PointerButton::Left => MouseButton::Left,
        PointerButton::Right => MouseButton::Right,
    }
}
