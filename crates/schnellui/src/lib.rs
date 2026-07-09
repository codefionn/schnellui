//! # schnellui
//!
//! The umbrella crate (SOUL §8): re-exports the pillars and provides [`App`] — the
//! one-shot, headless application object the screenshotter examples drive
//! (SOUL §7). `App::mount(root)` builds the retained tree once; `App::frame()` runs
//! the **pull → layout → paint → a11y** pass order over the dirty sets (SOUL §8.1);
//! and the introspection surface (`set_signal`, `render_to_png`, `dump_a11y`,
//! `dispatch_action`) is what makes the framework legible to an AI agent
//! (Directive #5, §7).

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

mod debug_server;
pub mod interaction_debug;
mod remount;
mod state;
mod structural_update;

pub use interaction_debug::{InteractionTrace, Remount};
pub use state::State;
pub use structural_update::{MissingSubtreeTarget, SubtreeReplacement, WindowUpdate};

pub use schnellui_a11y as a11y;
pub use schnellui_charts as charts;
pub use schnellui_icons as icons;
pub use schnellui_layout as layout;
/// Locale negotiation and application-owned message catalogs.
pub use schnellui_localization as localization;
pub use schnellui_render_wgpu as render;
pub use schnellui_scene as scene;
pub use schnellui_signal as signal;
pub use schnellui_store as store;
pub use schnellui_store::{Selector, Store};
pub use schnellui_template as template;
pub use schnellui_text as text;
pub use schnellui_theme as theme;
pub use schnellui_widgets as widgets;

/// The `view!` macro (SOUL §3.3).
pub use schnellui_macro::view;
pub use schnellui_platform::{self as platform, ColorScheme};

/// Exact platform-independent key identity types used by [`RawKeyEvent`].
/// Applications do not need a separate direct winit dependency to match them.
pub mod raw_keyboard {
    pub use winit::keyboard::{
        Key, KeyCode, KeyLocation, NamedKey, NativeKey, NativeKeyCode, PhysicalKey,
    };
}

use schnellui_layout::LayoutEngine;
use schnellui_render_wgpu::{Backend, Renderer, SurfaceRenderer};
use schnellui_scene::{Color, Point, Scene, Size, WidgetId, WidgetKind};
use schnellui_template::{DriveAction, Template};
use schnellui_text::{GlyphAtlas, TextShaper};
use schnellui_widgets::{BuildCtx, Theme, View};

pub use schnellui_widgets::Context;
pub use windowed::RedrawSignal;

/// One wheel notch of vertical scroll, in logical pixels (SOUL §3.2 scroll). Both an
/// inbound AccessKit `ScrollUp`/`ScrollDown` action ([`App::dispatch_action`]) and a
/// mouse-wheel `LineDelta` notch (windowed mode) move the viewport by this much.
const SCROLL_STEP: f32 = 48.0;

/// How a themed application chooses its root design system.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ThemeMode {
    /// Always use this theme.
    Fixed(Theme),
    /// Follow the native window's light/dark appearance.
    ///
    /// Headless applications and platforms that cannot report an appearance use
    /// `light`. Native `ThemeChanged` events switch to the corresponding theme.
    System { light: Theme, dark: Theme },
}

impl ThemeMode {
    pub const fn system(light: Theme, dark: Theme) -> ThemeMode {
        ThemeMode::System { light, dark }
    }

    fn resolve(self, scheme: ColorScheme) -> Theme {
        match self {
            ThemeMode::Fixed(theme) => theme,
            ThemeMode::System { light, dark } => match scheme {
                ColorScheme::Light => light,
                ColorScheme::Dark => dark,
            },
        }
    }
}

impl From<Theme> for ThemeMode {
    fn from(theme: Theme) -> Self {
        ThemeMode::Fixed(theme)
    }
}

#[derive(Clone, Copy, Debug)]
struct ActiveThemeTransition {
    from: Theme,
    to: Theme,
    started: Instant,
    duration: Duration,
}

/// A test-injectable value routed through the test-signal registry (SOUL §7 —
/// `App::set_signal`). Covers the scalar types scenarios drive.
#[derive(Clone, Debug, PartialEq)]
pub enum TestValue {
    Int(i64),
    Float(f64),
    Bool(bool),
    Text(String),
}

impl From<i32> for TestValue {
    fn from(v: i32) -> Self {
        TestValue::Int(v as i64)
    }
}
impl From<i64> for TestValue {
    fn from(v: i64) -> Self {
        TestValue::Int(v)
    }
}
impl From<f64> for TestValue {
    fn from(v: f64) -> Self {
        TestValue::Float(v)
    }
}
impl From<bool> for TestValue {
    fn from(v: bool) -> Self {
        TestValue::Bool(v)
    }
}
impl From<&str> for TestValue {
    fn from(v: &str) -> Self {
        TestValue::Text(v.to_string())
    }
}
impl From<String> for TestValue {
    fn from(v: String) -> Self {
        TestValue::Text(v)
    }
}

/// A registered test-signal setter: applies an injected [`TestValue`] to a signal
/// via its typed `set` (SOUL §7).
type TestSetter = Box<dyn FnMut(TestValue) + 'static>;

/// A platform-independent application keyboard shortcut.
///
/// `command` is the platform's primary command modifier: Ctrl on Linux and
/// Windows, and Cmd on macOS. Character keys are matched case-insensitively;
/// callers use [`Shortcut::command`] for the common single-modifier form.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Shortcut {
    key: char,
    command: bool,
    shift: bool,
    alt: bool,
}

impl Shortcut {
    /// Creates a shortcut from a character and resolved platform-independent
    /// modifiers.
    pub const fn new(key: char, command: bool, shift: bool, alt: bool) -> Self {
        Self {
            key,
            command,
            shift,
            alt,
        }
    }

    /// Creates a shortcut using only the platform command modifier.
    pub const fn command(key: char) -> Self {
        Self::new(key, true, false, false)
    }

    fn normalized(mut self) -> Self {
        self.key = self.key.to_ascii_lowercase();
        self
    }
}

type ShortcutHandler = Box<dyn FnMut() + 'static>;
type FocusedKeyHandler = Box<dyn for<'a> FnMut(UiKey<'a>) -> bool + 'static>;
type FocusedInputHandler = Box<dyn FnMut(FocusedInputEvent) -> FocusedInputResult + 'static>;
type CursorProvider = Box<dyn Fn() -> widgets::CursorIcon + 'static>;

struct FocusedKeyBinding {
    role: u16,
    name: Option<String>,
    handler: FocusedKeyHandler,
}

struct FocusedInputBinding {
    role: u16,
    name: Option<String>,
    handler: FocusedInputHandler,
}

struct CursorBinding {
    widget: WidgetId,
    provider: CursorProvider,
}

/// Keyboard modifier state attached to raw focused input events.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RawModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub super_key: bool,
}

/// Whether a raw key or pointer button is being pressed or released.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RawInputState {
    Pressed,
    Released,
}

/// A lossless, owned keyboard event for focused surfaces such as terminals.
///
/// The key identity types are winit's platform-independent keyboard types. This
/// preserves named, function, keypad, native and unidentified keys without
/// forcing SchnellUI to maintain a second, inevitably incomplete key-code enum.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawKeyEvent {
    pub logical_key: winit::keyboard::Key,
    pub physical_key: winit::keyboard::PhysicalKey,
    pub key_without_modifiers: winit::keyboard::Key,
    pub location: winit::keyboard::KeyLocation,
    pub modifiers: RawModifiers,
    pub state: RawInputState,
    /// `true` only for an auto-repeated press. Releases are never repeats.
    pub repeat: bool,
    /// Text produced by the key according to the active layout.
    pub text: Option<String>,
    /// Text produced with every modifier applied, including Ctrl/Alt.
    pub text_with_all_modifiers: Option<String>,
}

/// A mouse button in a raw focused pointer event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RawPointerButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    Other(u16),
}

/// Native wheel units. Pixel values have already been converted to logical
/// pixels; line values retain the platform's row/notch units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RawWheelDelta {
    Lines { x: f32, y: f32 },
    Pixels { x: f32, y: f32 },
}

/// The pointer action carried by a [`RawPointerEvent`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RawPointerAction {
    Move,
    Button {
        button: RawPointerButton,
        state: RawInputState,
    },
    Wheel(RawWheelDelta),
}

/// Pointer input offered to the focused raw surface.
///
/// `position` is relative to the focused widget's top-left corner, while
/// `window_position` is in SchnellUI logical window coordinates. A captured
/// drag may report a local position outside the widget bounds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RawPointerEvent {
    pub position: Point,
    pub window_position: Point,
    pub modifiers: RawModifiers,
    pub action: RawPointerAction,
}

/// Focus transitions delivered to a raw focused surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RawFocusEvent {
    WidgetGained,
    WidgetLost,
    WindowGained,
    WindowLost,
}

/// Input-method-editor state for composed/international text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RawImeEvent {
    Enabled,
    Disabled,
    Preedit {
        text: String,
        cursor: Option<(usize, usize)>,
    },
    Commit(String),
}

/// Clipboard operation offered to a focused raw surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FocusedClipboardEvent {
    /// Request the surface's currently selected text.
    Copy,
    /// Paste native clipboard text into the surface.
    Paste(String),
}

/// Complete low-level input stream for a focused application-defined surface.
#[derive(Clone, Debug, PartialEq)]
pub enum FocusedInputEvent {
    Key(RawKeyEvent),
    Pointer(RawPointerEvent),
    Focus(RawFocusEvent),
    Ime(RawImeEvent),
    Clipboard(FocusedClipboardEvent),
}

/// Result of handling a [`FocusedInputEvent`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum FocusedInputResult {
    #[default]
    Ignored,
    Handled,
    /// Handles a copy request and asks the native host to own this text.
    CopyText(String),
}

impl FocusedInputResult {
    pub fn is_handled(&self) -> bool {
        !matches!(self, Self::Ignored)
    }
}

/// One UI key press with its modifiers already resolved by the caller — the
/// windowing-toolkit-agnostic keyboard event [`App::dispatch_key`] routes with
/// **standard browser semantics** (SOUL §6.3). The windowed loop translates winit
/// `KeyEvent`s into this; tests and agents construct it directly (Directive #5).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UiKey<'a> {
    /// Tab / Shift+Tab — walk the a11y tab order.
    Tab {
        shift: bool,
    },
    Enter,
    Space {
        shift: bool,
    },
    Backspace,
    Delete,
    Left {
        shift: bool,
        ctrl: bool,
    },
    Right {
        shift: bool,
        ctrl: bool,
    },
    Up {
        shift: bool,
    },
    Down {
        shift: bool,
    },
    Home {
        shift: bool,
    },
    End {
        shift: bool,
    },
    PageUp,
    PageDown,
    /// Dismisses the top-most dismissible dialog.
    Escape,
    /// The platform select-all command (Ctrl+A, or Cmd+A on macOS).
    SelectAll,
    /// A control-modified ASCII character, used by raw keyboard surfaces such
    /// as terminal emulators (for example Ctrl+C → ETX).
    Control(char),
    /// Typed text (layout- and shift-resolved, never control characters).
    Char(&'a str),
}

/// A keyboard Home/End pins a scroll viewport to its start/end: a delta so large
/// [`dispatch_scroll`](widgets::dispatch_scroll)'s clamp lands exactly on the
/// boundary.
const SCROLL_TO_END: f32 = f32::MAX / 2.0;

/// The headless application (SOUL §7). Owns the retained scene, the layout/text
/// engines, the glyph atlas, an optional GPU renderer, and the test-signal
/// registry. Constructed empty via [`App::new`] or with a mounted root via
/// [`App::mount`].
pub struct App {
    context: Context,
    /// Non-`Send` retained widget behavior owned and dispatched by this app.
    widgets: schnellui_widgets::Runtime,
    scene: Scene,
    layout: LayoutEngine,
    text: TextShaper,
    atlas: GlyphAtlas,
    renderer: Option<Renderer>,
    /// name → setter, populated by widgets/scenarios so an agent can drive state
    /// by name (SOUL §7, `--assert`/`--scenario`).
    test_registry: HashMap<String, TestSetter>,
    /// Application commands keyed by a platform-independent chord. The native
    /// host resolves winit modifiers before dispatch; headless tests can drive
    /// the same path directly.
    shortcut_registry: HashMap<Shortcut, ShortcutHandler>,
    /// Raw key handlers scoped to a focused semantic surface. Applications use
    /// this for controls such as terminal emulators that consume keyboard input
    /// directly instead of editing through a text-input widget.
    focused_key_bindings: Vec<FocusedKeyBinding>,
    /// Full-fidelity input handlers for focused raw surfaces. Unlike the
    /// compatibility key handlers above, these receive releases, pointer,
    /// focus, IME and clipboard events.
    focused_input_bindings: Vec<FocusedInputBinding>,
    /// Dynamic cursor sources for application-defined surfaces such as embedded
    /// browser and terminal viewports. Bindings are scoped to one mounted subtree.
    cursor_bindings: Vec<CursorBinding>,
    /// logical viewport (SOUL §7.3 fixed viewport).
    size: Size,
    /// logical→physical scale (SOUL §7.1 `--scale`). Text is shaped/rasterized at
    /// `size_px * scale`; the PNG target is `width*scale × height*scale` physical
    /// pixels. `1.0` for the standard deterministic shot (SOUL §7.3).
    scale: f32,
    /// the fixed background clear color the renderer paints under the scene
    /// (SOUL §7.3). Defaults to opaque white so black text is legible.
    clear: Color,
    /// injected logical clock; always 0 for deterministic shots (SOUL §7.3).
    now: u64,
    /// whether a layout pass has run at least once (so `frame` lays out on first
    /// frame even with no layout-dirty entry — mount precedes the first layout).
    laid_out: bool,
    /// reactive paint bindings: `(node, || -> Color)`. Evaluated in the pull phase
    /// of every frame; each writes ONE node's fill via `scene.set_color` (PAINT-only,
    /// idempotent). Because the closure only reads `Copy` signal values and returns a
    /// `Copy` `Color`, and `set_color` mutates columns in place, a steady-state frame
    /// over these bindings allocates **nothing** — the literal-zero `rerender_1_signal`
    /// path of the covenant (SOUL §1, §4.1), the non-text sibling of the widgets'
    /// dynamic-text slots (the budgeted `text_edit` path).
    paint_bindings: Vec<(WidgetId, Box<dyn FnMut() -> Color + 'static>)>,
    /// Reconstructs a themed retained tree. Present only for `mount_themed*`;
    /// ordinary one-shot mounts keep their original zero-overhead lifecycle.
    theme_factory: Option<Box<dyn FnMut() -> Box<dyn View>>>,
    theme_mode: ThemeMode,
    color_scheme: ColorScheme,
    active_theme: Theme,
    theme_transition_duration: Duration,
    theme_transition: Option<ActiveThemeTransition>,
    theme_binding: Option<Box<dyn FnMut() -> Theme>>,
    /// Whether motion is allowed by the host platform's accessibility preference.
    /// Headless/custom hosts default to `true` and can update this through
    /// [`App::apply_reduced_motion`]; the built-in window host does so automatically.
    animations_enabled: bool,
    /// Whether the native host should continuously request frames. Streaming
    /// surfaces such as terminals use this to present data arriving on worker
    /// threads without coupling those threads to winit.
    continuous_redraw: bool,
    /// Optional paced native redraw used by embedders that need periodic polling
    /// without presenting at the monitor refresh rate while idle.
    redraw_interval: Option<Duration>,
    /// Thread-safe native wake handle. It is inert for headless applications and
    /// connected to the winit proxy when a window starts.
    redraw_signal: RedrawSignal,
    /// Optional native window-title source for dynamic application metadata
    /// such as an OSC 0/2 terminal title.
    window_title_provider: Option<Box<dyn FnMut() -> String + 'static>>,
    /// Explicit native interaction tracing configuration. The window host also
    /// accepts `SCHNELLUI_INTERACTION_TRACE` when this is absent.
    interaction_trace: Option<InteractionTrace>,
}

#[derive(Clone, Debug)]
struct DialogGeometry {
    panel: WidgetId,
    reference: Option<scene::ComponentRef>,
    role: u16,
    name: Option<String>,
    anchor: Option<Point>,
    width: Option<f32>,
    height: Option<f32>,
}

#[derive(Clone, Copy, Debug)]
struct RemountFocus {
    target: WidgetId,
    ring_visible: bool,
}

fn owning_combo_trigger(scene: &Scene, option: WidgetId) -> Option<WidgetId> {
    let mut ancestor = scene.node(option).and_then(|node| node.parent);
    while let Some(id) = ancestor {
        if let Some(trigger) = scene.node(id).and_then(|node| {
            node.children.iter().copied().find(|child| {
                scene.a11y(*child).is_some_and(|semantics| {
                    a11y::Role::from_u16(semantics.role) == a11y::Role::ComboBox
                })
            })
        }) {
            return Some(trigger);
        }
        ancestor = scene.node(id).and_then(|node| node.parent);
    }
    None
}

mod app_input;
mod app_interaction;
mod app_lifecycle;
mod app_render;
/// Re-export of the AccessKit action types used by [`App::dispatch_action`], so
/// callers need not depend on `accesskit` directly (SOUL §6.3).
pub mod accesskit_action {
    pub use schnellui_a11y::accesskit_reexport::{Action, ActionData, ActionRequest};
}

/// Re-export of the AccessKit tree/id types (SOUL §6.2) so callers can build
/// targets (`to_access_id`) and consume [`App::a11y_tree_update`] without a direct
/// `accesskit` dependency.
pub mod accesskit_reexport {
    pub use schnellui_a11y::accesskit_reexport::*;
}

#[cfg(test)]
mod tests_core;
#[cfg(test)]
mod tests_navigation;
#[cfg(test)]
mod tests_remount;
/// The opt-in windowed (non-headless) event loop (SOUL §8). A thin winit-0.30
/// `ApplicationHandler` translating window events into the [`App`]'s frame/input path
/// with **reactive** redraw scheduling (Directive #3). Headless mode never enters here.
mod windowed;
