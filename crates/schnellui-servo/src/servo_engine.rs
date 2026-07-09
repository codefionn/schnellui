//! Concrete Servo 0.4 browser engine adapter.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::path::Path;
use std::rc::Rc;
use std::str::FromStr;
use std::time::{Duration, Instant};

use cookie::Cookie;
use schnellui::widgets::CursorIcon;
use servo::{
    Code, CompositionEvent, CompositionState, CookieSource, DeviceIntRect, DeviceIntSize,
    DevicePoint, ImeEvent, InputEvent, Key, KeyState, KeyboardEvent, Location, Modifiers,
    MouseButton, MouseButtonAction, MouseButtonEvent, MouseMoveEvent, Opts, RenderingContext,
    Servo, ServoBuilder, SoftwareRenderingContext, WebView, WebViewBuilder, WheelDelta, WheelEvent,
    WheelMode,
};
use url::Url;

use crate::{
    BrowserEngine, BrowserFrame, BrowserInput, BrowserKeyEvent, BrowserMouseButton,
    BrowserPointerEvent, BrowserPointerKind, BrowserTabState, BrowserWheelDelta, PersistedCookie,
};

const SCROLL_METRICS_INTERVAL: Duration = Duration::from_millis(100);
const WHEEL_LINE_PIXELS: f32 = 76.0;

#[derive(Clone, Debug, Default)]
struct PageMetadata {
    url: Option<Url>,
    title: Option<String>,
    history: Vec<Url>,
    history_index: usize,
    crashed: Option<String>,
    frame_ready: bool,
    scroll_x: f64,
    scroll_y: f64,
    content_height: f64,
    viewport_height: f64,
    active: bool,
}

struct PageDelegate {
    metadata: Rc<RefCell<PageMetadata>>,
}

impl servo::WebViewDelegate for PageDelegate {
    fn notify_url_changed(&self, _webview: WebView, url: Url) {
        let mut metadata = self.metadata.borrow_mut();
        metadata.url = Some(url);
        metadata.scroll_x = 0.0;
        metadata.scroll_y = 0.0;
        metadata.content_height = 0.0;
        metadata.viewport_height = 0.0;
    }

    fn notify_page_title_changed(&self, _webview: WebView, title: Option<String>) {
        self.metadata.borrow_mut().title = title;
    }

    fn notify_history_changed(&self, _webview: WebView, entries: Vec<Url>, current: usize) {
        let mut metadata = self.metadata.borrow_mut();
        metadata.history = entries;
        metadata.history_index = current;
    }

    fn notify_new_frame_ready(&self, _webview: WebView) {
        self.metadata.borrow_mut().frame_ready = true;
    }

    fn notify_crashed(&self, _webview: WebView, reason: String, _backtrace: Option<String>) {
        self.metadata.borrow_mut().crashed = Some(reason);
    }
}

pub struct ServoTabHandle {
    webview: WebView,
    context: Rc<SoftwareRenderingContext>,
    metadata: Rc<RefCell<PageMetadata>>,
    composing: Cell<bool>,
    scroll_metrics_pending: Rc<Cell<bool>>,
    scroll_metrics_requested_at: Cell<Option<Instant>>,
}

pub struct ServoEngine {
    servo: Servo,
    width: u32,
    height: u32,
}

impl ServoEngine {
    /// Creates one Servo instance. Every tab opened by this adapter shares the
    /// instance's public cookie jar, HTTP cache and storage threads.
    pub fn new(width: u32, height: u32) -> Result<Self, ServoEngineError> {
        Self::new_with_builder(width, height, ServoBuilder::default())
    }

    /// Creates one Servo instance backed by a durable browser profile.
    ///
    /// Servo stores cookies, local storage, client storage, authentication
    /// state, and other profile-owned site data below `profile_path`. Session
    /// storage remains scoped to the lifetime of the Servo session, matching
    /// normal browser semantics.
    pub fn new_with_profile(
        width: u32,
        height: u32,
        profile_path: impl AsRef<Path>,
    ) -> Result<Self, ServoEngineError> {
        let profile_path = profile_path.as_ref();
        std::fs::create_dir_all(profile_path).map_err(|source| {
            ServoEngineError::ProfileDirectory {
                path: profile_path.to_path_buf(),
                source,
            }
        })?;
        let options = Opts {
            config_dir: Some(profile_path.to_path_buf()),
            temporary_storage: false,
            ..Opts::default()
        };
        Self::new_with_builder(width, height, ServoBuilder::default().opts(options))
    }

    fn new_with_builder(
        width: u32,
        height: u32,
        builder: ServoBuilder,
    ) -> Result<Self, ServoEngineError> {
        if width == 0 || height == 0 {
            return Err(ServoEngineError::InvalidViewport);
        }
        let mut preferences = servo::Preferences::default();
        if cfg!(debug_assertions) {
            // Stylo's debug worker stack is intentionally small and can overflow
            // on ordinary pages. Keep debug embeds reliable; optimized release
            // builds retain Servo's parallel styling default.
            preferences.layout_threads = 1;
        }
        Ok(Self {
            servo: builder.preferences(preferences).build(),
            width,
            height,
        })
    }

    pub fn servo(&self) -> &Servo {
        &self.servo
    }

    pub fn resize(&self, handle: &ServoTabHandle, width: u32, height: u32) {
        let size = dpi::PhysicalSize::new(width.max(1), height.max(1));
        if handle.context.size() != size {
            handle.webview.resize(size);
        }
    }

    pub fn page_crash(&self, handle: &ServoTabHandle) -> Option<String> {
        handle.metadata.borrow().crashed.clone()
    }

    pub fn load_status(&self, handle: &ServoTabHandle) -> servo::LoadStatus {
        handle.webview.load_status()
    }

    pub fn is_load_complete(&self, handle: &ServoTabHandle) -> bool {
        handle.webview.load_status() == servo::LoadStatus::Complete
    }

    /// Whether Servo notified the embedder that this tab has a new frame to paint.
    /// Reading the software framebuffer without this signal repeats an expensive
    /// full-viewport copy and can starve the host application's event loop.
    pub fn frame_ready(&self, handle: &ServoTabHandle) -> bool {
        handle.metadata.borrow().frame_ready
    }

    pub fn is_focused(&self, handle: &ServoTabHandle) -> bool {
        handle.webview.focused()
    }

    pub fn evaluate_javascript(
        &self,
        handle: &ServoTabHandle,
        script: impl ToString,
        callback: impl FnOnce(Result<servo::JSValue, servo::JavaScriptEvaluationError>) + 'static,
    ) {
        handle.webview.evaluate_javascript(script, callback);
    }

    /// Scrolls the root document to an absolute CSS-pixel offset. Servo 0.4
    /// does not paint classic page scrollbars, so native hosts use this to keep
    /// their scrollbar track synchronized with the web content.
    pub fn scroll_to(&self, handle: &ServoTabHandle, x: f64, y: f64) {
        let x = finite_non_negative(x);
        let y = finite_non_negative(y);
        handle
            .webview
            .evaluate_javascript(format!("window.scrollTo({x}, {y})"), |_| {});
    }

    fn request_scroll_metrics(handle: &ServoTabHandle) {
        let now = Instant::now();
        if handle
            .scroll_metrics_requested_at
            .get()
            .is_some_and(|requested| now.duration_since(requested) < SCROLL_METRICS_INTERVAL)
        {
            return;
        }
        if handle.scroll_metrics_pending.replace(true) {
            return;
        }
        handle.scroll_metrics_requested_at.set(Some(now));
        let metadata = Rc::clone(&handle.metadata);
        let pending = Rc::clone(&handle.scroll_metrics_pending);
        handle.webview.evaluate_javascript(
            "(() => { const d = document.documentElement; const b = document.body; return { x: Math.max(0, window.scrollX), y: Math.max(0, window.scrollY), contentHeight: Math.max(d ? d.scrollHeight : 0, b ? b.scrollHeight : 0, window.innerHeight), viewportHeight: window.innerHeight }; })()",
            move |result| {
                if let Ok(servo::JSValue::Object(values)) = result {
                    let number = |key| match values.get(key) {
                        Some(servo::JSValue::Number(value)) if value.is_finite() => Some(*value),
                        _ => None,
                    };
                    let mut metadata = metadata.borrow_mut();
                    if let Some(value) = number("x") {
                        metadata.scroll_x = value.max(0.0);
                    }
                    if let Some(value) = number("y") {
                        metadata.scroll_y = value.max(0.0);
                    }
                    if let Some(value) = number("contentHeight") {
                        metadata.content_height = value.max(0.0);
                    }
                    if let Some(value) = number("viewportHeight") {
                        metadata.viewport_height = value.max(0.0);
                    }
                }
                pending.set(false);
            },
        );
    }
}

fn finite_non_negative(value: f64) -> f64 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

fn cursor_icon(cursor: servo::Cursor) -> CursorIcon {
    match cursor {
        servo::Cursor::None => CursorIcon::None,
        servo::Cursor::Default => CursorIcon::Default,
        servo::Cursor::Pointer => CursorIcon::Pointer,
        servo::Cursor::ContextMenu => CursorIcon::ContextMenu,
        servo::Cursor::Help => CursorIcon::Help,
        servo::Cursor::Progress => CursorIcon::Progress,
        servo::Cursor::Wait => CursorIcon::Wait,
        servo::Cursor::Cell => CursorIcon::Cell,
        servo::Cursor::Crosshair => CursorIcon::Crosshair,
        servo::Cursor::Text => CursorIcon::Text,
        servo::Cursor::VerticalText => CursorIcon::VerticalText,
        servo::Cursor::Alias => CursorIcon::Alias,
        servo::Cursor::Copy => CursorIcon::Copy,
        servo::Cursor::Move => CursorIcon::Move,
        servo::Cursor::NoDrop => CursorIcon::NoDrop,
        servo::Cursor::NotAllowed => CursorIcon::NotAllowed,
        servo::Cursor::Grab => CursorIcon::Grab,
        servo::Cursor::Grabbing => CursorIcon::Grabbing,
        servo::Cursor::EResize => CursorIcon::EResize,
        servo::Cursor::NResize => CursorIcon::NResize,
        servo::Cursor::NeResize => CursorIcon::NeResize,
        servo::Cursor::NwResize => CursorIcon::NwResize,
        servo::Cursor::SResize => CursorIcon::SResize,
        servo::Cursor::SeResize => CursorIcon::SeResize,
        servo::Cursor::SwResize => CursorIcon::SwResize,
        servo::Cursor::WResize => CursorIcon::WResize,
        servo::Cursor::EwResize => CursorIcon::EwResize,
        servo::Cursor::NsResize => CursorIcon::NsResize,
        servo::Cursor::NeswResize => CursorIcon::NeswResize,
        servo::Cursor::NwseResize => CursorIcon::NwseResize,
        servo::Cursor::ColResize => CursorIcon::ColResize,
        servo::Cursor::RowResize => CursorIcon::RowResize,
        servo::Cursor::AllScroll => CursorIcon::AllScroll,
        servo::Cursor::ZoomIn => CursorIcon::ZoomIn,
        servo::Cursor::ZoomOut => CursorIcon::ZoomOut,
    }
}

impl BrowserEngine for ServoEngine {
    type TabHandle = ServoTabHandle;
    type Error = ServoEngineError;

    fn open_tab(&mut self, state: &BrowserTabState) -> Result<Self::TabHandle, Self::Error> {
        let context = Rc::new(
            SoftwareRenderingContext::new(dpi::PhysicalSize::new(self.width, self.height))
                .map_err(|error| ServoEngineError::RenderingContext(format!("{error:?}")))?,
        );
        let metadata = Rc::new(RefCell::new(PageMetadata::default()));
        let delegate = Rc::new(PageDelegate {
            metadata: metadata.clone(),
        });
        let webview = WebViewBuilder::new(&self.servo, context.clone())
            .url(state.url.clone())
            .delegate(delegate)
            .build();
        webview.set_page_zoom(state.zoom);
        Ok(ServoTabHandle {
            webview,
            context,
            metadata,
            composing: Cell::new(false),
            scroll_metrics_pending: Rc::new(Cell::new(false)),
            scroll_metrics_requested_at: Cell::new(None),
        })
    }

    fn close_tab(&mut self, _handle: Self::TabHandle) {}

    fn set_active(&mut self, handle: &Self::TabHandle, active: bool) {
        handle.metadata.borrow_mut().active = active;
        if active {
            handle.webview.show();
            handle.webview.set_throttled(false);
            handle.webview.focus();
        } else {
            handle.webview.blur();
            handle.webview.set_throttled(true);
            handle.webview.hide();
        }
    }

    fn navigate(&mut self, handle: &Self::TabHandle, url: &Url) {
        handle.webview.load(url.clone());
    }

    fn go_back(&mut self, handle: &Self::TabHandle, restored_target: &Url) {
        if handle.webview.can_go_back() {
            handle.webview.go_back(1);
        } else {
            handle.webview.load(restored_target.clone());
        }
    }

    fn go_forward(&mut self, handle: &Self::TabHandle, restored_target: &Url) {
        if handle.webview.can_go_forward() {
            handle.webview.go_forward(1);
        } else {
            handle.webview.load(restored_target.clone());
        }
    }

    fn reload(&mut self, handle: &Self::TabHandle) {
        handle.webview.reload();
    }

    fn set_zoom(&mut self, handle: &Self::TabHandle, zoom: f32) {
        handle.webview.set_page_zoom(zoom);
    }

    fn dispatch_input(&mut self, handle: &Self::TabHandle, input: BrowserInput) {
        match input {
            BrowserInput::Focus(true) => handle.webview.focus(),
            BrowserInput::Focus(false) => handle.webview.blur(),
            BrowserInput::Key(event) => {
                handle
                    .webview
                    .notify_input_event(InputEvent::Keyboard(keyboard_event(event)));
            }
            BrowserInput::Pointer(event) => dispatch_pointer(&handle.webview, event),
            BrowserInput::Composition { text, committed } => {
                if !handle.composing.replace(!committed) {
                    handle
                        .webview
                        .notify_input_event(InputEvent::Ime(ImeEvent::Composition(
                            CompositionEvent {
                                state: CompositionState::Start,
                                data: String::new(),
                            },
                        )));
                }
                handle
                    .webview
                    .notify_input_event(InputEvent::Ime(ImeEvent::Composition(CompositionEvent {
                        state: if committed {
                            CompositionState::End
                        } else {
                            CompositionState::Update
                        },
                        data: text,
                    })));
            }
        }
    }

    fn spin_event_loop(&mut self) {
        self.servo.spin_event_loop();
    }

    fn cursor(&self, handle: &Self::TabHandle) -> CursorIcon {
        cursor_icon(handle.webview.cursor())
    }

    fn sync_state(&mut self, handle: &Self::TabHandle, state: &mut BrowserTabState) {
        {
            let metadata = handle.metadata.borrow();
            if let Some(url) = &metadata.url {
                state.url.clone_from(url);
            }
            if let Some(title) = &metadata.title {
                state.title.clone_from(title);
            }
            if !metadata.history.is_empty() && metadata.history_index < metadata.history.len() {
                state.history.clone_from(&metadata.history);
                state.history_index = metadata.history_index;
            }
            state.scroll_x = metadata.scroll_x;
            state.scroll_y = metadata.scroll_y;
            state.content_height = metadata.content_height;
            state.viewport_height = metadata.viewport_height;
        }
        state.zoom = handle.webview.page_zoom();
        if handle.metadata.borrow().active
            && handle.webview.load_status() == servo::LoadStatus::Complete
        {
            Self::request_scroll_metrics(handle);
        }
    }

    fn render(&mut self, handle: &Self::TabHandle) -> Option<BrowserFrame> {
        if !handle.metadata.borrow().frame_ready {
            return None;
        }
        let size = handle.context.size();
        handle.webview.paint();
        let image = handle
            .context
            .read_to_image(DeviceIntRect::from_size(DeviceIntSize::new(
                size.width as i32,
                size.height as i32,
            )))?;
        handle.metadata.borrow_mut().frame_ready = false;
        Some(BrowserFrame {
            width: image.width(),
            height: image.height(),
            rgba: image.into_raw(),
        })
    }

    fn restore_cookies(&mut self, cookies: &[PersistedCookie]) -> Result<(), Self::Error> {
        for persisted in cookies {
            let cookie = Cookie::parse(persisted.set_cookie.clone())
                .map_err(|error| ServoEngineError::Cookie(error.to_string()))?
                .into_owned();
            self.servo.site_data_manager().set_cookie_for_url(
                persisted.origin.clone(),
                cookie,
                None,
            );
        }
        Ok(())
    }

    fn snapshot_cookies(&mut self, origins: &[Url]) -> Result<Vec<PersistedCookie>, Self::Error> {
        let mut seen = BTreeSet::new();
        let mut persisted = Vec::new();
        for origin in origins {
            for cookie in self
                .servo
                .site_data_manager()
                .cookies_for_url(origin.clone(), CookieSource::HTTP)
            {
                let set_cookie = cookie.to_string();
                if seen.insert((origin.clone(), set_cookie.clone())) {
                    persisted.push(PersistedCookie {
                        origin: origin.clone(),
                        set_cookie,
                    });
                }
            }
        }
        Ok(persisted)
    }
}

fn keyboard_event(event: BrowserKeyEvent) -> KeyboardEvent {
    let key = Key::from_str(&event.key).unwrap_or(Key::Character(event.key));
    let code = Code::from_str(&event.code).unwrap_or(Code::Unidentified);
    let mut modifiers = Modifiers::empty();
    modifiers.set(Modifiers::SHIFT, event.modifiers.shift);
    modifiers.set(Modifiers::CONTROL, event.modifiers.control);
    modifiers.set(Modifiers::ALT, event.modifiers.alt);
    modifiers.set(Modifiers::META, event.modifiers.meta);
    KeyboardEvent::new_without_event(
        if event.pressed {
            KeyState::Down
        } else {
            KeyState::Up
        },
        key,
        code,
        Location::Standard,
        modifiers,
        event.repeat,
        false,
    )
}

fn dispatch_pointer(webview: &WebView, event: BrowserPointerEvent) {
    let point = DevicePoint::new(event.x, event.y).into();
    let input = match event.kind {
        BrowserPointerKind::Move => InputEvent::MouseMove(MouseMoveEvent::new(point)),
        BrowserPointerKind::Button { button, pressed } => {
            InputEvent::MouseButton(MouseButtonEvent::new(
                if pressed {
                    MouseButtonAction::Down
                } else {
                    MouseButtonAction::Up
                },
                mouse_button(button),
                point,
            ))
        }
        BrowserPointerKind::Wheel(delta) => {
            let (x, y, mode) = match delta {
                BrowserWheelDelta::Lines { x, y } => (
                    x * WHEEL_LINE_PIXELS,
                    y * WHEEL_LINE_PIXELS,
                    WheelMode::DeltaLine,
                ),
                BrowserWheelDelta::Pixels { x, y } => (x, y, WheelMode::DeltaPixel),
            };
            InputEvent::Wheel(WheelEvent::new(
                WheelDelta {
                    x: f64::from(x),
                    y: f64::from(y),
                    z: 0.0,
                    mode,
                },
                point,
            ))
        }
    };
    webview.notify_input_event(input);
}

const fn mouse_button(button: BrowserMouseButton) -> MouseButton {
    match button {
        BrowserMouseButton::Left => MouseButton::Left,
        BrowserMouseButton::Right => MouseButton::Right,
        BrowserMouseButton::Middle => MouseButton::Middle,
        BrowserMouseButton::Back => MouseButton::Back,
        BrowserMouseButton::Forward => MouseButton::Forward,
        BrowserMouseButton::Other(value) => MouseButton::Other(value),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ServoEngineError {
    #[error("viewport dimensions must be non-zero")]
    InvalidViewport,
    #[error("failed to create Servo profile directory {path}: {source}")]
    ProfileDirectory {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to create Servo rendering context: {0}")]
    RenderingContext(String),
    #[error("invalid persisted cookie: {0}")]
    Cookie(String),
}
