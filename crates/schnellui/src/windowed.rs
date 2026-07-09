use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use accesskit_winit::{
    Adapter as AccessKitAdapter, Event as AccessKitEvent, WindowEvent as AccessKitWindowEvent,
};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::window::{CursorIcon as WinitCursorIcon, Theme as WinitTheme, Window, WindowId};

use serde_json::{json, Value};

use crate::platform::{watch_preferences, PreferenceChange, SystemClipboard};
use schnellui_render_wgpu::{Backend, SurfaceRenderer};
use schnellui_scene::{Point, Size, WidgetId as SceneWidgetId, WidgetKind};
use schnellui_widgets::TextPointerAction;
use schnellui_widgets::{CursorIcon as UiCursorIcon, DragRelease as UiDragRelease};

use super::{
    App, FocusedClipboardEvent, FocusedInputEvent, FocusedInputResult, RawFocusEvent, RawImeEvent,
    RawInputState, RawKeyEvent, RawModifiers, RawPointerAction, RawPointerButton, RawWheelDelta,
    Shortcut, UiKey,
};
use crate::debug_server::{
    DebugAction, DebugCommand, DebugKey, DebugPoint, DebugReply, DebugRequest, DebugServer,
    DebugTarget,
};
use crate::interaction_debug::InteractionRecorder;
use crate::{Remount, SubtreeReplacement, WindowUpdate};

#[derive(Debug)]
enum PlatformEvent {
    AccessKit(AccessKitEvent),
    ReducedMotionChanged(bool),
    RedrawRequested,
    Debug(DebugRequest),
}

/// A clonable native-window wake handle suitable for PTY and other worker
/// threads. The platform proxy remains encapsulated inside SchnellUI.
#[derive(Default)]
struct RedrawSignalInner {
    proxy: Mutex<Option<EventLoopProxy<PlatformEvent>>>,
    pending: AtomicBool,
}

#[derive(Clone, Default)]
pub struct RedrawSignal(Arc<RedrawSignalInner>);

impl RedrawSignal {
    fn install(&self, proxy: EventLoopProxy<PlatformEvent>) {
        if let Ok(mut installed) = self.0.proxy.lock() {
            *installed = Some(proxy);
        }
    }

    pub fn request_redraw(&self) {
        if self.0.pending.swap(true, Ordering::AcqRel) {
            return;
        }
        let mut sent = false;
        if let Ok(installed) = self.0.proxy.lock() {
            if let Some(proxy) = installed.as_ref() {
                sent = proxy.send_event(PlatformEvent::RedrawRequested).is_ok();
            }
        }
        if !sent {
            self.0.pending.store(false, Ordering::Release);
        }
    }

    /// Marks the controller wake as consumed when its resulting native redraw
    /// actually begins. Keeping the wake pending until this point lets all
    /// structural updates coalesce into that frame.
    fn acknowledge_native_redraw(&self) {
        self.0.pending.store(false, Ordering::Release);
    }
}

impl From<AccessKitEvent> for PlatformEvent {
    fn from(event: AccessKitEvent) -> Self {
        Self::AccessKit(event)
    }
}

const MULTI_CLICK_INTERVAL: Duration = Duration::from_millis(500);
const MULTI_CLICK_SLOP: f32 = 5.0;

fn raw_modifiers(modifiers: ModifiersState, physical_control: bool) -> RawModifiers {
    RawModifiers {
        shift: modifiers.shift_key(),
        control: modifiers.control_key() || physical_control,
        alt: modifiers.alt_key(),
        super_key: modifiers.super_key(),
    }
}

fn raw_input_state(state: ElementState) -> RawInputState {
    match state {
        ElementState::Pressed => RawInputState::Pressed,
        ElementState::Released => RawInputState::Released,
    }
}

fn debug_label(value: impl std::fmt::Debug) -> String {
    format!("{value:?}").to_ascii_lowercase()
}

fn point_json(point: Point) -> Value {
    json!({ "x": point.x, "y": point.y })
}

fn widget_json(app: &App, id: SceneWidgetId) -> Value {
    let node = app.scene().node(id);
    let a11y = app.scene().a11y(id);
    let layout = app.scene().layout(id);
    json!({
        "id": schnellui_a11y::to_access_id(id).0,
        "kind": node.map(|node| debug_label(node.kind)),
        "role": a11y.map(|node| schnellui_a11y::Role::from_u16(node.role).label()),
        "name": a11y.and_then(|node| node.name.as_deref()),
        "value": a11y.and_then(|node| node.value.as_deref()),
        "actions": a11y.map(|node| schnellui_a11y::ActionFlags(node.actions).names()),
        "state": a11y.map(|node| schnellui_a11y::StateFlags(node.state).names()),
        "rect": layout.map(|layout| json!({
            "x": layout.rect.x,
            "y": layout.rect.y,
            "width": layout.rect.width,
            "height": layout.rect.height,
        })),
    })
}

/// Semantic leaf-to-root hit path at a logical point.
fn hit_path_json(app: &App, point: Point) -> Value {
    let mut path = Vec::new();
    let mut current = schnellui_widgets::hit_test(&app.widgets.clone(), app.scene(), point);
    while let Some(id) = current {
        path.push(widget_json(app, id));
        current = app.scene().node(id).and_then(|node| node.parent);
    }
    Value::Array(path)
}

fn focused_json(app: &App) -> Value {
    app.focused_widget()
        .map(|id| widget_json(app, id))
        .unwrap_or(Value::Null)
}

fn window_event_name(event: &WindowEvent) -> &'static str {
    match event {
        WindowEvent::CloseRequested => "close_requested",
        WindowEvent::ModifiersChanged(_) => "modifiers_changed",
        WindowEvent::Focused(_) => "window_focus",
        WindowEvent::ThemeChanged(_) => "theme_changed",
        WindowEvent::KeyboardInput { .. } => "keyboard_input",
        WindowEvent::Ime(_) => "ime",
        WindowEvent::CursorMoved { .. } => "pointer_move",
        WindowEvent::CursorLeft { .. } => "pointer_left",
        WindowEvent::MouseInput { .. } => "pointer_button",
        WindowEvent::MouseWheel { .. } => "pointer_wheel",
        WindowEvent::Resized(_) => "resize",
        WindowEvent::RedrawRequested => "redraw",
        _ => "other_window_event",
    }
}

fn raw_pointer_button(button: MouseButton) -> RawPointerButton {
    match button {
        MouseButton::Left => RawPointerButton::Left,
        MouseButton::Right => RawPointerButton::Right,
        MouseButton::Middle => RawPointerButton::Middle,
        MouseButton::Back => RawPointerButton::Back,
        MouseButton::Forward => RawPointerButton::Forward,
        MouseButton::Other(value) => RawPointerButton::Other(value),
    }
}

fn control_letter_from_text(text: Option<&str>) -> Option<char> {
    let mut characters = text?.chars();
    let control = characters.next()?;
    if characters.next().is_some() || !(1..=26).contains(&u32::from(control)) {
        return None;
    }
    char::from_u32(u32::from('a') + u32::from(control) - 1)
}

fn resolve_control_letter(
    modifier_text: Option<&str>,
    logical_text: Option<&str>,
    ctrl: bool,
) -> Option<char> {
    if !ctrl {
        return None;
    }
    control_letter_from_text(modifier_text).or_else(|| {
        let mut characters = logical_text?.chars();
        let character = characters.next()?;
        if characters.next().is_some() {
            return None;
        }
        control_letter_from_text(logical_text).or(Some(character))
    })
}

fn is_control_key(logical: &Key, physical: PhysicalKey) -> bool {
    matches!(logical, Key::Named(NamedKey::Control))
        || matches!(
            physical,
            PhysicalKey::Code(KeyCode::ControlLeft | KeyCode::ControlRight)
        )
}

fn modifier_text_implies_control(text: Option<&str>, physical: PhysicalKey) -> bool {
    control_letter_from_text(text).is_some()
        && matches!(
            physical,
            PhysicalKey::Code(
                KeyCode::KeyA
                    | KeyCode::KeyB
                    | KeyCode::KeyC
                    | KeyCode::KeyD
                    | KeyCode::KeyE
                    | KeyCode::KeyF
                    | KeyCode::KeyG
                    | KeyCode::KeyH
                    | KeyCode::KeyI
                    | KeyCode::KeyJ
                    | KeyCode::KeyK
                    | KeyCode::KeyL
                    | KeyCode::KeyM
                    | KeyCode::KeyN
                    | KeyCode::KeyO
                    | KeyCode::KeyP
                    | KeyCode::KeyQ
                    | KeyCode::KeyR
                    | KeyCode::KeyS
                    | KeyCode::KeyT
                    | KeyCode::KeyU
                    | KeyCode::KeyV
                    | KeyCode::KeyW
                    | KeyCode::KeyX
                    | KeyCode::KeyY
                    | KeyCode::KeyZ
            )
        )
}

fn special_key_from_physical(key: PhysicalKey, shift: bool, ctrl: bool) -> Option<UiKey<'static>> {
    match key {
        PhysicalKey::Code(KeyCode::Tab) => Some(UiKey::Tab { shift }),
        PhysicalKey::Code(KeyCode::Enter | KeyCode::NumpadEnter) => Some(UiKey::Enter),
        PhysicalKey::Code(KeyCode::Space) => Some(UiKey::Space { shift }),
        PhysicalKey::Code(KeyCode::Backspace) => Some(UiKey::Backspace),
        PhysicalKey::Code(KeyCode::Delete) => Some(UiKey::Delete),
        PhysicalKey::Code(KeyCode::ArrowLeft) => Some(UiKey::Left { shift, ctrl }),
        PhysicalKey::Code(KeyCode::ArrowRight) => Some(UiKey::Right { shift, ctrl }),
        PhysicalKey::Code(KeyCode::ArrowUp) => Some(UiKey::Up { shift }),
        PhysicalKey::Code(KeyCode::ArrowDown) => Some(UiKey::Down { shift }),
        PhysicalKey::Code(KeyCode::Home) => Some(UiKey::Home { shift }),
        PhysicalKey::Code(KeyCode::End) => Some(UiKey::End { shift }),
        PhysicalKey::Code(KeyCode::PageUp) => Some(UiKey::PageUp),
        PhysicalKey::Code(KeyCode::PageDown) => Some(UiKey::PageDown),
        PhysicalKey::Code(KeyCode::Escape) => Some(UiKey::Escape),
        _ => None,
    }
}

#[derive(Clone, Copy)]
struct TextClick {
    target: SceneWidgetId,
    position: Point,
    at: Instant,
    count: u8,
}

/// Reads the optional `SCHNELLUI_AUTOCLOSE_MS` deadline (SOUL §8 smoke-test hook).
/// Returns `None` when unset/unparseable, so a real window stays open until the
/// user closes it.
fn autoclose_deadline() -> Option<Instant> {
    std::env::var("SCHNELLUI_AUTOCLOSE_MS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(|ms| Instant::now() + Duration::from_millis(ms))
}

const ANIMATION_REDRAW_INTERVAL: Duration = Duration::from_millis(16);

/// Turns an animation source into one immediate frame followed by paced wakes.
/// Calling `request_redraw` unconditionally from every `about_to_wait` callback
/// keeps winit runnable forever and can consume a full core when presentation is
/// not itself blocking on vsync.
fn pace_animation_redraw(
    active: bool,
    now: Instant,
    deadline: &mut Option<Instant>,
) -> (bool, Option<Instant>) {
    if !active {
        *deadline = None;
        return (false, None);
    }
    let due = deadline.is_none_or(|deadline| now >= deadline);
    if due {
        *deadline = Some(now + ANIMATION_REDRAW_INTERVAL);
    }
    (due, *deadline)
}

fn rebase_redraw_after_frame(
    deadline: &mut Option<Instant>,
    interval: Option<Duration>,
    now: Instant,
) {
    if deadline.is_some() {
        *deadline = interval.map(|interval| now + interval);
    }
}

/// The winit `ApplicationHandler` (SOUL §8): owns the mounted [`App`], the window,
/// and the surface renderer; it translates window events into the App's frame/input
/// path and schedules reactive redraws.
struct WindowedApp {
    app: App,
    /// Lazily opened system clipboard. Keeping it alive also preserves
    /// clipboard ownership on platforms where the source serves the data.
    clipboard: SystemClipboard,
    title: String,
    window: Option<Arc<Window>>,
    renderer: Option<SurfaceRenderer>,
    /// Native accessibility bridge. It is constructed while the window is
    /// still hidden, receives every winit event before the application, and
    /// publishes the retained AccessKit tree to AT-SPI/UIA/NSAccessibility.
    accessibility: Option<AccessKitAdapter>,
    event_loop_proxy: EventLoopProxy<PlatformEvent>,
    /// Geometry changes and remounts require a full tree update; ordinary
    /// semantic changes use the scene's proportional a11y-dirty update.
    accessibility_full_update_pending: bool,
    /// last known cursor position in **physical** pixels.
    cursor: PhysicalPosition<f64>,
    /// Last cursor sent to the window, avoiding redundant platform calls.
    cursor_icon: UiCursorIcon,
    /// Structured JSONL recorder; absent is a zero-work steady state.
    interaction_trace: Option<InteractionRecorder>,
    /// Localhost command bridge, present by default only in debug builds.
    _debug_server: Option<DebugServer>,
    /// Stable event category whose callback caused the latest remount poll.
    remount_trigger: &'static str,
    /// Monotonic count of accepted structural remounts.
    remount_count: u64,
    /// Monotonic count of accepted retained subtree replacements.
    subtree_replacement_count: u64,
    /// Monotonic remount counts grouped by the host-provided stable reason.
    remount_counts_by_reason: BTreeMap<String, u64>,
    /// Reason and triggering event for the latest accepted remount.
    last_remount: Option<(String, &'static str)>,
    /// initial window size in physical pixels (logical × scale).
    init_phys: PhysicalSize<u32>,
    /// optional auto-exit deadline (SOUL §8 smoke-test hook).
    deadline: Option<Instant>,
    /// Next paced redraw deadline, reset when the mounted app disables it.
    interval_deadline: Option<Instant>,
    /// Shared frame deadline for continuous/animated widget redraw sources.
    animation_deadline: Option<Instant>,
    /// live keyboard modifier state (shift/ctrl/…, SOUL §6.3 keyboard path).
    modifiers: ModifiersState,
    /// Physical Ctrl state backs up `ModifiersChanged`, which can lag a key
    /// event on some Wayland compositors. Terminal control chords must never
    /// degrade into printable characters.
    control_pressed: bool,
    /// Buttons whose press was consumed by a raw focused surface. Their
    /// move/release stream remains captured even outside its bounds.
    raw_pointer_capture: Vec<RawPointerButton>,
    /// the text input a left-button drag started on — `CursorMoved` extends its
    /// selection until the button releases (SOUL §6.3 pointer-selection path).
    drag_text: Option<SceneWidgetId>,
    /// Last editable press used to recognize native-style double/triple clicks.
    last_text_click: Option<TextClick>,
    /// The slider captured by a left-button press. Pointer movement scrubs its
    /// value even after the cursor leaves the original hit box.
    drag_slider: Option<SceneWidgetId>,
    /// Tracks the ordinary left-button stream for opt-in pointer-edge scrolling.
    left_pointer_down: bool,
    /// The host's structural update hook. Ordinary input polls it immediately;
    /// scroll input defers it to the native redraw so a wheel burst coalesces.
    remount: Option<Box<dyn FnMut(schnellui_scene::Size) -> Option<WindowUpdate>>>,
}

mod event_handler;
mod host;
pub(crate) use host::run;
#[cfg(test)]
mod tests;
