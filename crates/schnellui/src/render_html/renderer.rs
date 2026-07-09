use super::*;
use crate::render_html::scripts::RECONCILE_SCRIPT;
#[cfg(not(target_arch = "wasm32"))]
use crate::render_html::scripts::{drive_script, patch_call_params, BrowserProfile, ConfigError};
use crate::render_html::template::{document, normalize_scale, stylesheet, DomTemplate};

// Native HTML renderer for schnellui templates.
//
// This backend emits semantic DOM elements and CSS. It deliberately contains no
// `<canvas>` element and does not translate WGPU paint primitives. Screenshots are
// captured by launching Chromium through `chromiumoxide`, loading the generated
// document, and asking the browser for a PNG.
//
// The opt-in `ssr` feature adds a typed server-rendering chain and hydration
// handoff. It is deliberately absent from default builds.

#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};

#[cfg(not(target_arch = "wasm32"))]
use chromiumoxide::browser::{Browser, BrowserConfig};
#[cfg(not(target_arch = "wasm32"))]
use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
#[cfg(not(target_arch = "wasm32"))]
use chromiumoxide::cdp::js_protocol::runtime::{AddBindingParams, EventBindingCalled};
#[cfg(not(target_arch = "wasm32"))]
use chromiumoxide::handler::viewport::Viewport;
#[cfg(not(target_arch = "wasm32"))]
use chromiumoxide::page::ScreenshotParams;
#[cfg(not(target_arch = "wasm32"))]
use futures::StreamExt;
use schnellui::template::Template;
use schnellui::widgets::Theme;
use serde::Deserialize;
use std::sync::{Arc, OnceLock};

pub use schnellui::template::{DriveAction, DriveTarget};

/// A complete HTML document produced by [`HtmlRenderer`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HtmlDocument {
    source: String,
}

pub(crate) enum RustHandler {
    Click(Box<dyn FnMut() + 'static>),
    Toggle(Box<dyn FnMut(bool) + 'static>),
    Change(Box<dyn FnMut(f32) + 'static>),
    Input(Box<dyn FnMut(&str) + 'static>),
}

/// The theme-dependent stylesheet, built once per renderer. A page's
/// stylesheet is `head` + its responsive rules + `tail`.
#[derive(Debug)]
pub(crate) struct StyleBase {
    head: String,
    tail: String,
}

/// Stands in for the responsive rules while the base stylesheet is generated.
/// It cannot occur in CSS.
const RESPONSIVE_MARKER: &str = "\u{0}sui-responsive\u{0}";

impl StyleBase {
    fn new(width: u32, height: u32, theme: Theme) -> Self {
        let sheet = stylesheet(width, height, theme, RESPONSIVE_MARKER);
        let (head, tail) = sheet
            .split_once(RESPONSIVE_MARKER)
            .expect("the stylesheet embeds the responsive rules once");
        Self {
            head: head.to_string(),
            tail: tail.to_string(),
        }
    }

    fn compose(&self, responsive_css: &str) -> String {
        let mut sheet =
            String::with_capacity(self.head.len() + responsive_css.len() + self.tail.len());
        sheet.push_str(&self.head);
        sheet.push_str(responsive_css);
        sheet.push_str(&self.tail);
        sheet
    }
}

/// One render of a view. The document is only assembled when asked for: the
/// live update path needs just the body and a stylesheet hash.
pub(crate) struct RenderedPage {
    /// Markup inside `#schnellui-root`.
    body: String,
    responsive_css: String,
    pub(crate) styles: Arc<OnceLock<StyleBase>>,
    pub(crate) handlers: Vec<RustHandler>,
}

impl RenderedPage {
    pub(crate) fn body(&self) -> &str {
        &self.body
    }

    pub(crate) fn style(&self) -> String {
        self.styles
            .get()
            .expect("styles are initialized when a page is rendered")
            .compose(&self.responsive_css)
    }

    /// Identifies the stylesheet. Only the responsive rules vary between
    /// renders of one renderer, so they are all that is hashed.
    pub(crate) fn style_hash(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.responsive_css.hash(&mut hasher);
        hasher.finish()
    }

    pub(crate) fn document(&self) -> HtmlDocument {
        HtmlDocument {
            source: document(&self.style(), &self.body),
        }
    }

    pub(crate) fn into_document(self) -> HtmlDocument {
        self.document()
    }

    /// The stylesheet to send with an update, or `None` if `sent` is already
    /// the stylesheet this page needs.
    pub(crate) fn style_update(&self, sent: Option<u64>) -> Option<String> {
        (sent != Some(self.style_hash())).then(|| self.style())
    }
}

impl HtmlDocument {
    pub fn as_str(&self) -> &str {
        &self.source
    }

    pub fn into_string(self) -> String {
        self.source
    }

    #[cfg(feature = "ssr")]
    pub(crate) fn source_mut(&mut self) -> &mut String {
        &mut self.source
    }
}

/// Native DOM/CSS renderer plus Chromium screenshot configuration.
#[derive(Clone, Debug)]
pub struct HtmlRenderer {
    width: u32,
    height: u32,
    scale: f32,
    theme: Theme,
    styles: Arc<OnceLock<StyleBase>>,
    #[cfg(not(target_arch = "wasm32"))]
    chrome_executable: Option<PathBuf>,
}

impl HtmlRenderer {
    /// Creates a renderer for a logical viewport. `scale` controls the physical
    /// screenshot size while component layout remains in logical CSS pixels.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width: width.max(1),
            height: height.max(1),
            scale: 1.0,
            theme: Theme::default(),
            styles: Arc::default(),
            #[cfg(not(target_arch = "wasm32"))]
            chrome_executable: None,
        }
    }

    pub fn set_scale(&mut self, scale: f32) {
        self.scale = normalize_scale(scale);
    }

    pub fn with_scale(mut self, scale: f32) -> Self {
        self.set_scale(scale);
        self
    }

    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
        self.styles = Arc::default();
    }

    pub fn with_theme(mut self, theme: Theme) -> Self {
        self.set_theme(theme);
        self
    }

    /// Overrides Chromiumoxide's automatic Chrome/Chromium discovery.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn set_chrome_executable(&mut self, path: impl Into<PathBuf>) {
        self.chrome_executable = Some(path.into());
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn with_chrome_executable(mut self, path: impl Into<PathBuf>) -> Self {
        self.set_chrome_executable(path);
        self
    }

    /// Renders a generic template into a self-contained native HTML document.
    pub fn render<V: Template>(&self, view: V) -> HtmlDocument {
        self.render_page(view).into_document()
    }

    pub(crate) fn render_page<V: Template>(&self, view: V) -> RenderedPage {
        let mut dom = DomTemplate::default();
        let body = view.render(&mut dom);
        for reference in &dom.queried_refs {
            let id = reference.id();
            let _ = writeln!(
                dom.responsive_css,
                r#"[data-sui-ref="{id}"] {{ container-name: sui-ref-{id}; container-type: size; }}"#
            );
        }
        let body = dom.display(body).to_string();
        self.styles
            .get_or_init(|| StyleBase::new(self.width, self.height, self.theme));
        RenderedPage {
            body,
            responsive_css: dom.responsive_css,
            styles: Arc::clone(&self.styles),
            handlers: dom.handlers,
        }
    }

    /// Starts a headless Chromium sized for this renderer's viewport, with its
    /// own profile directory.
    #[cfg(not(target_arch = "wasm32"))]
    async fn launch_browser(
        &self,
    ) -> Result<LaunchedBrowser, Box<dyn std::error::Error + Send + Sync>> {
        let physical_width = ((self.width as f32) * self.scale).round().max(1.0) as u32;
        let physical_height = ((self.height as f32) * self.scale).round().max(1.0) as u32;
        let profile = BrowserProfile::create()?;
        let mut config = BrowserConfig::builder()
            .window_size(physical_width, physical_height)
            .user_data_dir(profile.path())
            .viewport(Viewport {
                width: self.width,
                height: self.height,
                device_scale_factor: Some(self.scale as f64),
                emulating_mobile: false,
                is_landscape: self.width > self.height,
                has_touch: false,
            })
            .new_headless_mode();
        if let Some(executable) = &self.chrome_executable {
            config = config.chrome_executable(executable);
        }
        let config = config.build().map_err(ConfigError)?;
        let (browser, mut handler) = Browser::launch(config).await?;
        let handler_task = tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if event.is_err() {
                    break;
                }
            }
        });
        Ok(LaunchedBrowser {
            browser,
            handler_task,
            _profile: profile,
        })
    }

    /// Captures a PNG with Chromiumoxide. The returned bytes are also written to
    /// `path` by Chromiumoxide.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn render_to_png<V, P>(
        &self,
        view: V,
        path: P,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>>
    where
        V: Template,
        P: AsRef<Path>,
    {
        let html = self.render(view);
        let launched = self.launch_browser().await?;

        let capture = async {
            let page = launched.browser.new_page("about:blank").await?;
            page.set_content(html.as_str()).await?;
            page.save_screenshot(
                ScreenshotParams::builder()
                    .format(CaptureScreenshotFormat::Png)
                    .full_page(false)
                    .omit_background(false)
                    .build(),
                path,
            )
            .await
        }
        .await;

        let close_result = launched.close().await;
        let bytes = capture?;
        close_result?;
        Ok(bytes)
    }

    /// Drives a renderer-generic view through real Chromium DOM events, invokes
    /// the corresponding Rust callbacks, renders the expected state from `view`,
    /// reconciles it into the live DOM, then captures PNG.
    ///
    /// This mirrors retained/WGPU scenarios: targets are accessibility role+name
    /// and each action settles synchronously before the next one. Reconciliation
    /// preserves matching DOM nodes, focus, text selection, and scroll state; it
    /// also verifies the patched root is structurally equal to the expected root.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn render_scenario<F, V, P>(
        &self,
        mut view: F,
        actions: &[DriveAction],
        path: P,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnMut() -> V,
        V: Template,
        P: AsRef<Path>,
    {
        let launched = self.launch_browser().await?;

        let result: Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> = async {
            let page = launched.browser.new_page("about:blank").await?;
            let mut binding_events = page.event_listener::<EventBindingCalled>().await?;
            page.execute(AddBindingParams::new(RUST_BINDING)).await?;
            let mut rendered = self.render_page(view());
            // The page is loaded with its own stylesheet; patches only send one
            // when the view's stylesheet differs from it.
            let mut sent_style = Some(rendered.style_hash());
            page.set_content(rendered.document().as_str()).await?;
            page.evaluate(RECONCILE_SCRIPT).await?;

            for action in actions {
                let script = drive_script(action);
                let dispatch: BrowserDispatch = page.evaluate(script).await?.into_value()?;
                if !dispatch.found {
                    return Err(format!("HTML scenario target not found: {action:?}").into());
                }
                if dispatch.callback {
                    let event = binding_events
                        .next()
                        .await
                        .ok_or("Chromium callback event stream closed")?;
                    if event.name != RUST_BINDING {
                        return Err(format!("unexpected Chromium binding: {}", event.name).into());
                    }
                    let payload: BindingPayload = serde_json::from_str(&event.payload)?;
                    let callback = rendered
                        .handlers
                        .get_mut(payload.id)
                        .ok_or_else(|| format!("unknown HTML callback id {}", payload.id))?;
                    invoke_handler(callback, &payload)?;

                    rendered = self.render_page(view());
                    let style = rendered.style_update(sent_style);
                    let diff: DomDiffResult = page
                        .evaluate_function(patch_call_params(rendered.body(), style.as_deref()))
                        .await?
                        .into_value()?;
                    sent_style = Some(rendered.style_hash());
                    if !diff.matches {
                        return Err(format!(
                            "HTML DOM reconciliation did not reach the expected state: {diff:?}"
                        )
                        .into());
                    }
                }
            }

            let bytes = page
                .save_screenshot(
                    ScreenshotParams::builder()
                        .format(CaptureScreenshotFormat::Png)
                        .full_page(false)
                        .omit_background(false)
                        .build(),
                    path,
                )
                .await?;
            Ok(bytes)
        }
        .await;

        let close_result = launched.close().await;
        let bytes = result?;
        close_result?;
        Ok(bytes)
    }

    /// Mounts a live renderer-generic application into Trunk's
    /// `#schnellui-root` element.
    ///
    /// Browser events call the same Rust handlers as the Chromium screenshot
    /// path. After each callback, the expected view is rendered and reconciled
    /// into the live DOM without replacing the document.
    #[cfg(target_arch = "wasm32")]
    pub fn mount<F, V>(&self, mut view: F) -> Result<WasmMount, wasm_bindgen::JsValue>
    where
        F: FnMut() -> V + 'static,
        V: Template + 'static,
    {
        let factory: WasmViewFactory = Box::new(move |renderer| renderer.render_page(view()));
        self.mount_rendered_factory(factory)
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn mount_rendered_factory(
        &self,
        mut factory: WasmViewFactory,
    ) -> Result<WasmMount, wasm_bindgen::JsValue> {
        use std::cell::RefCell;
        use std::rc::Rc;

        use wasm_bindgen::closure::Closure;
        use wasm_bindgen::JsValue;

        let renderer = self.clone();
        let rendered = factory(&renderer);
        let patch = wasm_install_patch()?;
        let mut sent_style = None;
        wasm_patch(&patch, &rendered, &mut sent_style)
            .map_err(|error| JsValue::from_str(&format!("initial {error}")))?;

        let session = Rc::new(RefCell::new(WasmSession {
            renderer,
            factory,
            rendered,
            patch,
            sent_style,
        }));

        let callback_session = Rc::clone(&session);
        let callback = Closure::<dyn FnMut(String)>::new(move |payload: String| {
            if let Err(error) = wasm_dispatch(&callback_session, &payload) {
                wasm_console_error(&error);
            }
        });
        js_sys::Reflect::set(
            &js_sys::global(),
            &JsValue::from_str(RUST_BINDING),
            callback.as_ref(),
        )?;
        Ok(WasmMount {
            session,
            _callback: callback,
            router_callback: None,
        })
    }
}

#[cfg(target_arch = "wasm32")]
type WasmViewFactory = Box<dyn FnMut(&HtmlRenderer) -> RenderedPage>;

#[cfg(target_arch = "wasm32")]
pub(crate) type WasmCallback = wasm_bindgen::closure::Closure<dyn FnMut(String)>;

#[cfg(target_arch = "wasm32")]
pub(crate) struct WasmSession {
    renderer: HtmlRenderer,
    factory: WasmViewFactory,
    rendered: RenderedPage,
    /// The page's `__schnelluiPatch` function, installed once at mount.
    patch: js_sys::Function,
    /// Hash of the stylesheet the page currently has.
    sent_style: Option<u64>,
}

/// Owns a live browser mount and its JavaScript callbacks.
///
/// Dropping this value unmounts the Rust event bindings. Applications normally
/// keep it in their own runtime state. A `#[wasm_bindgen(start)]` entry point
/// that intentionally lives for the page lifetime can call [`forget`](Self::forget).
#[cfg(target_arch = "wasm32")]
pub struct WasmMount {
    // Held so the session lives as long as the mount.
    #[allow(dead_code)]
    pub(crate) session: std::rc::Rc<std::cell::RefCell<WasmSession>>,
    _callback: WasmCallback,
    #[allow(dead_code)]
    pub(crate) router_callback: Option<WasmCallback>,
}

#[cfg(target_arch = "wasm32")]
impl WasmMount {
    /// Keeps this mount alive for the remainder of the browser page lifetime.
    pub fn forget(self) {
        std::mem::forget(self);
    }
}

/// A running Chromium plus the task that drives its event stream. The profile
/// directory is removed when this is dropped.
#[cfg(not(target_arch = "wasm32"))]
struct LaunchedBrowser {
    browser: Browser,
    handler_task: tokio::task::JoinHandle<()>,
    _profile: BrowserProfile,
}

#[cfg(not(target_arch = "wasm32"))]
impl LaunchedBrowser {
    async fn close(mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let closed = self.browser.close().await;
        let _ = self.handler_task.await;
        closed?;
        Ok(())
    }
}

pub(crate) const RUST_BINDING: &str = "__schnellui_rust_event";

#[derive(Debug, Deserialize)]
pub(crate) struct BindingPayload {
    pub(crate) id: usize,
    #[serde(default)]
    pub(crate) value: String,
    #[serde(default)]
    pub(crate) checked: bool,
}

#[derive(Debug, Deserialize)]
#[cfg(not(target_arch = "wasm32"))]
struct BrowserDispatch {
    found: bool,
    callback: bool,
}

/// Diagnostics returned by the in-browser DOM reconciler.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[cfg(not(target_arch = "wasm32"))]
struct DomDiffResult {
    matches: bool,
    attributes: usize,
    text: usize,
    inserted: usize,
    removed: usize,
    replaced: usize,
    moved: usize,
}

pub(crate) fn invoke_handler(
    handler: &mut RustHandler,
    payload: &BindingPayload,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match handler {
        RustHandler::Click(callback) => callback(),
        RustHandler::Toggle(callback) => callback(payload.checked),
        RustHandler::Change(callback) => callback(payload.value.parse()?),
        RustHandler::Input(callback) => callback(&payload.value),
    }
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn wasm_dispatch(session: &std::cell::RefCell<WasmSession>, payload: &str) -> Result<(), String> {
    let mut session = session.borrow_mut();
    let payload: BindingPayload =
        serde_json::from_str(payload).map_err(|error| error.to_string())?;
    let handler = session
        .rendered
        .handlers
        .get_mut(payload.id)
        .ok_or_else(|| format!("unknown HTML callback id {}", payload.id))?;
    invoke_handler(handler, &payload).map_err(|error| error.to_string())?;
    wasm_rerender(&mut session)
}

#[cfg(all(feature = "ssr", target_arch = "wasm32"))]
pub(crate) fn wasm_rerender_session(
    session: &std::cell::RefCell<WasmSession>,
) -> Result<(), String> {
    wasm_rerender(&mut session.borrow_mut())
}

#[cfg(target_arch = "wasm32")]
fn wasm_rerender(session: &mut WasmSession) -> Result<(), String> {
    let renderer = session.renderer.clone();
    let rendered = (session.factory)(&renderer);
    wasm_patch(&session.patch, &rendered, &mut session.sent_style)?;
    session.rendered = rendered;
    Ok(())
}

/// Installs the reconciler once and returns its patch function.
#[cfg(target_arch = "wasm32")]
fn wasm_install_patch() -> Result<js_sys::Function, wasm_bindgen::JsValue> {
    use wasm_bindgen::JsCast;

    js_sys::eval(RECONCILE_SCRIPT)?;
    js_sys::Reflect::get(
        &js_sys::global(),
        &wasm_bindgen::JsValue::from_str(crate::render_html::scripts::PATCH_FUNCTION),
    )?
    .dyn_into::<js_sys::Function>()
    .map_err(|_| wasm_bindgen::JsValue::from_str("the DOM reconciler did not install"))
}

/// Reconciles the live DOM with `rendered`. Only the body markup and, if it
/// changed, the stylesheet cross into JavaScript.
#[cfg(target_arch = "wasm32")]
fn wasm_patch(
    patch: &js_sys::Function,
    rendered: &RenderedPage,
    sent_style: &mut Option<u64>,
) -> Result<(), String> {
    use wasm_bindgen::JsValue;

    let style = rendered
        .style_update(*sent_style)
        .map_or(JsValue::NULL, |style| JsValue::from_str(&style));
    let diff = patch
        .call2(&JsValue::NULL, &JsValue::from_str(rendered.body()), &style)
        .map_err(|error| format!("DOM reconciliation failed: {error:?}"))?;
    *sent_style = Some(rendered.style_hash());
    let matches = js_sys::Reflect::get(&diff, &JsValue::from_str("matches"))
        .map_err(|error| format!("cannot inspect DOM reconciliation: {error:?}"))?
        .as_bool()
        .unwrap_or(false);
    if matches {
        Ok(())
    } else {
        Err("DOM reconciliation did not reach the expected state".to_string())
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn wasm_console_error(message: &str) {
    let message = serde_json::to_string(message).expect("error message is serializable");
    let _ = js_sys::eval(&format!("console.error({message})"));
}
