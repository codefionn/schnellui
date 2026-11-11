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
use crate::debug_server::{DebugButton, DebugCommand, DebugDrag, DebugKey, DebugPoint, DebugPress};
use crate::scene::{Point, Rect, Size};
use crate::widgets::Theme;
use crate::{App, WindowUpdate};

/// A pointer button the headless host can press.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PointerButton {
    Left,
    Right,
}

/// How a [`HeadlessHost`] delivers pointer input.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PointerRoute {
    /// Straight into the window host's `CursorMoved`, `CursorLeft` and
    /// `MouseInput` handlers, then the remount poll and a frame.
    Handlers,
    /// Through the debug server's commands (`move`, `press`, `release`,
    /// `click-at`, `drag`, `leave`), as `schnellui-debug` drives an app
    /// started with `--headless-windowed`. Each command draws its own frame,
    /// so this route cannot batch.
    Debug,
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
    Undo,
    Redo,
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
            HostKey::Undo => "undo",
            HostKey::Redo => "redo",
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
    route: PointerRoute,
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
            route: PointerRoute::Handlers,
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
                backdrop_press: None,
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

    /// Chooses how pointer input reaches the app from here on.
    pub fn set_route(&mut self, route: PointerRoute) {
        self.route = route;
        if route == PointerRoute::Debug {
            self.set_batching(false);
        }
    }

    /// Runs one debug server command as the event loop does and returns the
    /// HTTP status of its reply. The command settles the app itself.
    fn debug(&mut self, command: DebugCommand) -> u16 {
        self.pin_clock();
        self.host.remount_trigger = "debug_instrumentation";
        let status = self.host.handle_debug_command(None, command).status;
        // A command that swapped in a remounted app left its clock unpinned.
        self.pin_clock();
        status
    }

    /// With batching on, input events are handled without a redraw between
    /// them, as a burst that arrives within one display frame is. Turning it
    /// off, or calling [`HeadlessHost::frame`], draws the frame. The debug
    /// route draws after every command and stays unbatched.
    pub fn set_batching(&mut self, batching: bool) {
        let batching = batching && self.route == PointerRoute::Handlers;
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

    /// Whether `button` is down.
    pub fn holding(&self, button: PointerButton) -> bool {
        self.held[button as usize]
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

    /// `p` moved inside the window, since a real pointer never reports a
    /// point outside it.
    fn inside(&self, p: Point) -> Point {
        let size = self.host.app.size();
        Point {
            x: p.x.clamp(0.0, (size.width - 1.0).max(0.0)),
            y: p.y.clamp(0.0, (size.height - 1.0).max(0.0)),
        }
    }

    /// Moves the pointer to a logical point. Points outside the window are
    /// clamped to its edge, since a real pointer never reports one.
    pub fn pointer_move(&mut self, p: Point) {
        let p = self.inside(p);
        if self.route == PointerRoute::Debug {
            self.debug(DebugCommand::PointerMove(DebugPoint { x: p.x, y: p.y }));
            return;
        }
        let scale = f64::from(self.host.app.scale());
        self.host.on_cursor_moved(PhysicalPosition::new(
            f64::from(p.x) * scale,
            f64::from(p.y) * scale,
        ));
        self.settle();
    }

    /// The pointer leaves the window.
    pub fn pointer_leave(&mut self) {
        if self.route == PointerRoute::Debug {
            self.debug(DebugCommand::PointerLeave);
            return;
        }
        let _ = self.host.on_cursor_left();
        self.settle();
    }

    /// Presses a button. Pressing one that is already held does nothing.
    pub fn press(&mut self, button: PointerButton) {
        if std::mem::replace(&mut self.held[button as usize], true) {
            return;
        }
        if self.route == PointerRoute::Debug {
            self.debug(DebugCommand::PointerPress(DebugPress {
                x: None,
                y: None,
                button: debug_button(button),
            }));
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
        if self.route == PointerRoute::Debug {
            self.debug(DebugCommand::PointerRelease(DebugPress {
                x: None,
                y: None,
                button: debug_button(button),
            }));
            return;
        }
        self.host
            .on_mouse_input(ElementState::Released, winit_button(button));
        self.settle();
    }

    /// Moves to `p` and presses there: the debug `press` command with a
    /// point, or a move and a press. Does nothing while the button is held.
    pub fn press_at(&mut self, p: Point, button: PointerButton) {
        if self.held[button as usize] {
            return;
        }
        if self.route == PointerRoute::Debug {
            let p = self.inside(p);
            self.held[button as usize] = true;
            self.debug(DebugCommand::PointerPress(DebugPress {
                x: Some(p.x),
                y: Some(p.y),
                button: debug_button(button),
            }));
            return;
        }
        self.pointer_move(p);
        self.press(button);
    }

    /// Moves to `p` and releases there: the debug `release` command with a
    /// point, or a move and a release. Does nothing while the button is up.
    pub fn release_at(&mut self, p: Point, button: PointerButton) {
        if !self.held[button as usize] {
            return;
        }
        if self.route == PointerRoute::Debug {
            let p = self.inside(p);
            self.held[button as usize] = false;
            self.debug(DebugCommand::PointerRelease(DebugPress {
                x: Some(p.x),
                y: Some(p.y),
                button: debug_button(button),
            }));
            return;
        }
        self.pointer_move(p);
        self.release(button);
    }

    /// A left click at `p`: the debug `click-at` command, or a move, a press
    /// and a release. Does nothing while the left button is held. Returns
    /// whether the click was delivered, which it is wherever the point is.
    pub fn click_at(&mut self, p: Point) -> bool {
        if self.held[PointerButton::Left as usize] {
            return false;
        }
        if self.route == PointerRoute::Debug {
            let p = self.inside(p);
            return self.debug(DebugCommand::PointerClick(DebugPoint { x: p.x, y: p.y })) == 200;
        }
        self.pointer_move(p);
        self.press(PointerButton::Left);
        self.release(PointerButton::Left);
        true
    }

    /// Presses at `from`, moves to `to` in `steps` equal steps and releases
    /// there: the debug `drag` command, or the same events one by one. Does
    /// nothing while the button is held.
    pub fn drag(&mut self, from: Point, to: Point, steps: u32, button: PointerButton) {
        if self.held[button as usize] {
            return;
        }
        let (from, to) = (self.inside(from), self.inside(to));
        let steps = steps.clamp(1, 1000);
        if self.route == PointerRoute::Debug {
            self.debug(DebugCommand::PointerDrag(DebugDrag {
                x1: from.x,
                y1: from.y,
                x2: to.x,
                y2: to.y,
                steps: Some(steps),
                button: debug_button(button),
            }));
            return;
        }
        self.pointer_move(from);
        self.press(button);
        for step in 1..=steps {
            let t = step as f32 / steps as f32;
            self.pointer_move(Point {
                x: from.x + (to.x - from.x) * t,
                y: from.y + (to.y - from.y) * t,
            });
        }
        self.release(button);
    }

    /// What the host holds of a pointer gesture under way: the `pointer`
    /// part of the debug server's status, as JSON text with its keys in
    /// alphabetical order. serde_json keeps insertion order instead when some
    /// crate in the build turns on its `preserve_order` feature, so the keys
    /// are sorted here and the text reads the same in every build.
    pub fn pointer_state(&self) -> String {
        let status = self.host.debug_status();
        match status["pointer"].as_object() {
            Some(pointer) => {
                let sorted: std::collections::BTreeMap<_, _> = pointer.iter().collect();
                serde_json::to_string(&sorted).unwrap_or_default()
            }
            None => status["pointer"].to_string(),
        }
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

    /// Presses Escape as the native window host does and returns whether it
    /// asked the window to close: no menu, list or dialog took the key and
    /// the app turned on [`App::set_escape_exits`]. An overlay that took it
    /// may remount, which is polled before this returns.
    pub fn press_escape(&mut self) -> bool {
        let exit = match self.host.dispatch_escape_key() {
            super::host::EscapeKey::Handled(_) => {
                self.host.poll_remount();
                false
            }
            super::host::EscapeKey::Exit => true,
            super::host::EscapeKey::Pass => false,
        };
        self.settle();
        exit
    }

    /// One wheel event of `delta_y` logical pixels at the pointer, positive
    /// scrolling down. It takes the window host's own wheel path: a focused
    /// raw surface and an open context menu see it first, then the deepest
    /// scroll viewport under the pointer, bubbling to its ancestors at an
    /// edge.
    pub fn wheel(&mut self, delta_y: f32) {
        // A window only gets wheel events while the pointer is over it.
        if self.pointer().is_none() {
            return;
        }
        let scale = f64::from(self.host.app.scale());
        let _ = self
            .host
            .on_mouse_wheel(winit::event::MouseScrollDelta::PixelDelta(
                PhysicalPosition::new(0.0, -f64::from(delta_y) * scale),
            ));
        self.settle();
    }

    /// Moves the pointer to `p`, then wheels there: a wheel over a point other
    /// than where the pointer rested.
    pub fn wheel_at(&mut self, p: Point, delta_y: f32) {
        self.pointer_move(p);
        self.wheel(delta_y);
    }

    /// Delivers an accessibility action as the platform bridge does, then
    /// settles. Returns whether a handler ran.
    pub fn dispatch_action(&mut self, request: &crate::accesskit_action::ActionRequest) -> bool {
        let acted = self.host.app.dispatch_action(request);
        self.settle();
        acted
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

    /// Forgets the last press, so the next one is a single click however soon
    /// and close to it comes. For replays that change how fast input arrives,
    /// where a double click would otherwise depend on the speed.
    pub fn forget_clicks(&mut self) {
        self.host.last_text_click = None;
    }

    /// Draws a frame with no input, even while batching.
    pub fn frame(&mut self) {
        self.pin_clock();
        self.host.poll_remount();
        self.pin_clock();
        self.host.on_redraw_requested();
    }
}

fn debug_button(button: PointerButton) -> DebugButton {
    match button {
        PointerButton::Left => DebugButton::Left,
        PointerButton::Right => DebugButton::Right,
    }
}

fn winit_button(button: PointerButton) -> MouseButton {
    match button {
        PointerButton::Left => MouseButton::Left,
        PointerButton::Right => MouseButton::Right,
    }
}
