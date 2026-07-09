//! # servo_demo — Servo web content embedded in SchnellUI
//!
//! Servo paints a webview into an offscreen RGBA frame. SchnellUI uploads that
//! frame through its ordinary `Image` widget and supplies the surrounding native
//! chrome. In `--windowed` mode, SchnellUI's full-fidelity focused-input path is
//! forwarded back to Servo. New browser frames stream into the retained image
//! (`Image::dynamic_rgba_versioned`), which overwrites its atlas region in place;
//! only navigation state, viewport and address-bar resets remount the app.
//! The address field is editable: Enter loads the typed address and Escape
//! restores the current page's URL.

use std::cell::{Cell, RefCell};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use clap::Parser;
use schnellui::a11y::Role;
use schnellui::raw_keyboard::{Key, NamedKey};
use schnellui::scene::{Color, Primitive, Rect, Size, WidgetId, WidgetKind};
use schnellui::widgets::{
    node_rect, Align, Badge, BuildCtx, Button, ButtonAppearance, Column, DynamicImageFrame, Flex,
    Image, Pad, Row, Scroll, Shape, Stack, Text, TextInput, Theme, View,
};
use schnellui::App;
use schnellui::{FocusedInputEvent, FocusedInputResult, RawInputState};
use schnellui_servo::servo_engine::ServoEngine;
use schnellui_servo::{
    Browser, BrowserFrame, BrowserInput, BrowserPointerKind, BrowserSessionState,
    BrowserStateStore, BrowserTabId, BrowserTabState, BrowserWheelDelta,
};
use url::Url;

const TAB_ID: &str = "servo-demo";
const SURFACE_NAME: &str = "Servo web content";
const ADDRESS_NAME: &str = "Address";
const SHELL_INSET: f32 = 22.0;
const CHROME_HEIGHT: f32 = 166.0;
const DEMO_PAGE: &str = include_str!("../page.html");

const INK: Color = Color::rgb(0x10, 0x2f, 0x2c);
const DEEP_INK: Color = Color::rgb(0x09, 0x20, 0x1f);
const RAISED_INK: Color = Color::rgb(0x17, 0x3d, 0x39);
const PAPER: Color = Color::rgb(0xf3, 0xed, 0xdf);
const MUTED: Color = Color::rgb(0x9c, 0xb0, 0xaa);
const SIGNAL: Color = Color::rgb(0xf0, 0x5a, 0x36);
const LINE: Color = Color::rgb(0x46, 0x68, 0x62);

const SERVO_THEME: Theme = Theme {
    text: PAPER,
    text_muted: MUTED,
    surface: INK,
    surface_muted: RAISED_INK,
    separator: LINE,
    outline: Color::rgb(0x70, 0x8b, 0x83),
    accent: SIGNAL,
    on_accent: DEEP_INK,
    selection: Color::rgb(0x31, 0x58, 0x51),
    interactions: schnellui::widgets::InteractionStates {
        hover: schnellui::widgets::InteractionStyle::all(
            Color::rgba(0x55, 0xd6, 0xc2, 0x20),
            PAPER,
            SIGNAL,
        ),
        focus: schnellui::widgets::InteractionStyle::border(SIGNAL),
        active: schnellui::widgets::InteractionStyle::background(Color::rgb(0x31, 0x58, 0x51)),
    },
    component_interactions: schnellui::widgets::ComponentInteractions::NONE,
    text_selection: Color::rgb(0xa9, 0x4c, 0x36),
    disabled: Color::rgb(0x50, 0x65, 0x61),
    positive: Color::rgb(0xa7, 0xc9, 0x73),
    attention: SIGNAL,
    media: Color::rgb(0x2b, 0x4b, 0x46),
    page: DEEP_INK,
    shape: Shape {
        roundness: 0.3,
        density: 0.85,
        frame: 1.0,
        shadow: 0.0,
    },
};

type LiveBrowser = Rc<RefCell<Browser<ServoEngine>>>;

/// The latest browser frame, shared by every mount of the app. The page image
/// reads it whenever `revision` moves, so a new frame never remounts.
struct FrameFeed {
    revision: Cell<u64>,
    frame: RefCell<DynamicImageFrame>,
}

impl FrameFeed {
    fn new(frame: BrowserFrame) -> Rc<Self> {
        Rc::new(Self {
            revision: Cell::new(0),
            frame: RefCell::new(DynamicImageFrame::new(
                frame.width,
                frame.height,
                frame.rgba,
            )),
        })
    }

    /// Publishes `frame` if its pixels differ from the current one.
    fn publish(&self, frame: BrowserFrame) {
        let unchanged = {
            let current = self.frame.borrow();
            current.width == frame.width
                && current.height == frame.height
                && *current.pixels == *frame.rgba
        };
        if unchanged {
            return;
        }
        *self.frame.borrow_mut() = DynamicImageFrame::new(frame.width, frame.height, frame.rgba);
        self.revision.set(self.revision.get() + 1);
    }

    /// The page surface: a retained image that re-reads the feed only when its
    /// revision changes and rewrites its existing atlas texels in place.
    fn image(self: &Rc<Self>) -> Image {
        let revision = self.clone();
        let frame = self.clone();
        Image::dynamic_rgba_versioned(
            move || revision.revision.get(),
            move || Some(frame.frame.borrow().clone()),
        )
    }
}

/// The address field's edit state. Navigation remounts the whole app, so the
/// typed text lives here rather than in a retained widget.
#[derive(Clone, Default)]
struct AddressBar {
    /// Text the user typed but has not submitted. `None` shows the tab's URL.
    draft: Rc<RefCell<Option<String>>>,
    /// Set when the field must show the tab's URL again without a new frame.
    reset: Rc<Cell<bool>>,
}

#[derive(Parser)]
#[command(name = "servo_demo", about = "render a Servo webview inside SchnellUI")]
struct Cli {
    /// Page to load. Without this flag, a deterministic built-in page is served locally.
    #[arg(long)]
    url: Option<Url>,
    /// Screenshot path used in headless mode.
    #[arg(long, default_value = "servo.png")]
    out: PathBuf,
    /// SchnellUI logical viewport width.
    #[arg(long, default_value_t = 960)]
    width: u32,
    /// SchnellUI logical viewport height.
    #[arg(long, default_value_t = 660)]
    height: u32,
    /// Logical-to-physical scale used by both renderers.
    #[arg(long, default_value_t = 1.0)]
    scale: f32,
    /// Maximum initial page-load wait.
    #[arg(long, default_value_t = 10_000)]
    timeout_ms: u64,
    /// Persist tabs, history, zoom, scroll state, and cookies at this path.
    #[arg(long)]
    state: Option<PathBuf>,
    /// Also dump SchnellUI's host accessibility tree.
    #[arg(long)]
    dump_a11y: Option<PathBuf>,
    /// Open an interactive native window instead of writing a PNG.
    #[arg(long)]
    windowed: bool,
    /// Run the live event loop offscreen without opening a desktop window.
    #[arg(long, conflicts_with = "windowed")]
    headless_windowed: bool,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("servo example failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    if cli.headless_windowed {
        std::env::set_var("SCHNELLUI_HEADLESS_WINDOWED", "1");
    }
    if cli.width < 320 || cli.height < 240 {
        return Err("viewport must be at least 320x240".into());
    }
    if !cli.scale.is_finite() || cli.scale <= 0.0 {
        return Err("scale must be finite and greater than zero".into());
    }

    // Rustls 0.23 requires a process-wide provider before Servo visits HTTPS URLs.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let server = if cli.url.is_none() {
        Some(DemoServer::start()?)
    } else {
        None
    };
    let target_url = cli
        .url
        .clone()
        .unwrap_or_else(|| server.as_ref().expect("demo server exists").url());
    let tab_id = BrowserTabId::new(TAB_ID)?;
    let logical_size = Size {
        width: cli.width as f32,
        height: cli.height as f32,
    };
    let (browser_width, browser_height) = browser_physical_size(logical_size, cli.scale);
    let engine = ServoEngine::new(browser_width, browser_height)?;
    let session = load_session(cli.state.as_ref())?;
    let had_tab = session.tabs.iter().any(|tab| tab.id == tab_id);
    let mut browser = Browser::restore(engine, session)?;
    if had_tab {
        if browser
            .tab(&tab_id)
            .is_some_and(|tab| tab.url != target_url)
        {
            browser.navigate(&tab_id, target_url)?;
        }
        browser.activate(&tab_id)?;
    } else {
        browser.open(BrowserTabState::new(tab_id.clone(), target_url))?;
    }

    let browser = Rc::new(RefCell::new(browser));
    let address = AddressBar::default();
    let initial_frame =
        wait_for_initial_frame(&browser, &tab_id, Duration::from_millis(cli.timeout_ms))?;
    let feed = FrameFeed::new(initial_frame);
    let mut app = browser_app(
        browser.clone(),
        tab_id.clone(),
        feed.clone(),
        address.clone(),
        logical_size,
        cli.scale,
        cli.windowed || cli.headless_windowed,
    );

    if cli.windowed || cli.headless_windowed {
        let live = browser.clone();
        let live_tab = tab_id.clone();
        let scale = cli.scale;
        let mut last_size = logical_size;
        let mut last_state = browser.borrow().tab(&tab_id).cloned();
        app.run_windowed_with_viewport("schnellui + Servo", move |viewport| {
            let (width, height) = browser_physical_size(viewport, scale);
            let mut browser = live.borrow_mut();
            let _ = browser.with_engine_tab(&live_tab, |engine, tab| {
                engine.resize(tab, width, height);
            });
            browser.spin_event_loop();
            let frame = browser.render(&live_tab).ok().flatten();
            let state = browser.tab(&live_tab).cloned();
            drop(browser);

            // A new frame streams into the mounted image; the redraw that follows
            // this hook polls the feed's revision. No remount.
            if let Some(frame) = frame {
                feed.publish(frame);
            }
            let changed = viewport != last_size || state != last_state || address.reset.take();
            if !changed {
                return None;
            }
            last_size = viewport;
            last_state = state;
            Some(browser_app(
                live.clone(),
                live_tab.clone(),
                feed.clone(),
                address.clone(),
                viewport,
                scale,
                true,
            ))
        })?;
    } else {
        app.frame();
        if let Some(path) = &cli.dump_a11y {
            app.dump_a11y(path)?;
        }
        app.render_to_png(&cli.out)?;
        println!("wrote {}", cli.out.display());
    }

    if let Some(path) = cli.state {
        let snapshot = browser.borrow_mut().snapshot()?;
        BrowserStateStore::new(path).save(&snapshot)?;
    }
    drop(server);
    Ok(())
}

fn load_session(path: Option<&PathBuf>) -> Result<BrowserSessionState, Box<dyn std::error::Error>> {
    match path {
        Some(path) => Ok(BrowserStateStore::new(path).load_or_default()?),
        None => Ok(BrowserSessionState::default()),
    }
}

fn browser_logical_size(viewport: Size) -> (f32, f32) {
    (
        (viewport.width - SHELL_INSET * 2.0 - 4.0).max(1.0),
        (viewport.height - CHROME_HEIGHT).max(1.0),
    )
}

fn browser_physical_size(viewport: Size, scale: f32) -> (u32, u32) {
    let (width, height) = browser_logical_size(viewport);
    (
        (width * scale).round().max(1.0) as u32,
        (height * scale).round().max(1.0) as u32,
    )
}

fn wait_for_initial_frame(
    browser: &LiveBrowser,
    tab_id: &BrowserTabId,
    timeout: Duration,
) -> Result<BrowserFrame, Box<dyn std::error::Error>> {
    let started = Instant::now();
    let mut loaded_at = None;
    let mut latest_frame = None;
    while started.elapsed() < timeout {
        let mut browser = browser.borrow_mut();
        browser.spin_event_loop();
        let frame = browser.render(tab_id)?;
        let loaded = browser.with_engine_tab(tab_id, |engine, tab| engine.is_load_complete(tab))?;
        let crash = browser.with_engine_tab(tab_id, |engine, tab| engine.page_crash(tab))?;
        drop(browser);
        if let Some(reason) = crash {
            return Err(format!("Servo page crashed: {reason}").into());
        }
        if loaded {
            loaded_at.get_or_insert_with(Instant::now);
            if frame.is_some() {
                latest_frame = frame;
            }
            // Load completion precedes Servo's final display-list paint. Give the
            // engine a handful of event-loop turns so the captured frame contains
            // the document rather than the webview's initial white clear.
            if loaded_at.is_some_and(|loaded| loaded.elapsed() >= Duration::from_millis(100)) {
                if let Some(frame) = latest_frame {
                    return Ok(frame);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    Err(format!(
        "page did not finish loading within {} ms",
        timeout.as_millis()
    )
    .into())
}

fn browser_app(
    browser: LiveBrowser,
    tab_id: BrowserTabId,
    feed: Rc<FrameFeed>,
    address: AddressBar,
    viewport: Size,
    scale: f32,
    continuous: bool,
) -> App {
    let state = browser
        .borrow()
        .tab(&tab_id)
        .cloned()
        .expect("the demo tab remains open");
    let can_go_back = state.history_index > 0;
    let can_go_forward = state.history_index + 1 < state.history.len();
    let (surface_width, surface_height) = browser_logical_size(viewport);

    let back_browser = browser.clone();
    let back_tab = tab_id.clone();
    let forward_browser = browser.clone();
    let forward_tab = tab_id.clone();
    let reload_browser = browser.clone();
    let reload_tab = tab_id.clone();
    let zoom_out_browser = browser.clone();
    let zoom_out_tab = tab_id.clone();
    let zoom_in_browser = browser.clone();
    let zoom_in_tab = tab_id.clone();
    let zoom = state.zoom;
    let chrome_width = surface_width + 4.0;
    let protocol = state.url.scheme().to_uppercase();
    let address_text = address
        .draft
        .borrow()
        .clone()
        .unwrap_or_else(|| state.url.as_str().to_owned());
    let address_draft = address.draft.clone();
    // Everything in the toolbar row except the address field has a fixed
    // width: three 34px and two 30px buttons, the protocol badge, the zoom
    // readout, seven 5px gaps and the 6px padding on each side.
    let address_width = (chrome_width - 12.0 - 102.0 - 60.0 - 48.0 - 36.0 - 35.0).max(120.0);
    let page_title = if state.title.trim().is_empty() {
        "Untitled document".to_owned()
    } else {
        state.title.clone()
    };

    let toolbar = Stack::new()
        .size(chrome_width, 42.0)
        .child(ChromeSurface::new(
            chrome_width,
            42.0,
            RAISED_INK,
            LINE,
            5.0,
            1.0,
        ))
        .child(
            Pad::all(6.0).child(
                Row::new()
                    .width(chrome_width - 12.0)
                    .gap(5.0)
                    .align(Align::Center)
                    .child(
                        Button::new("←")
                            .width(34.0)
                            .height(28.0)
                            .appearance(ButtonAppearance::Ghost)
                            .tooltip("Back")
                            .disabled(!can_go_back)
                            .on_click(move || {
                                let _ = back_browser.borrow_mut().go_back(&back_tab);
                            }),
                    )
                    .child(
                        Button::new("→")
                            .width(34.0)
                            .height(28.0)
                            .appearance(ButtonAppearance::Ghost)
                            .tooltip("Forward")
                            .disabled(!can_go_forward)
                            .on_click(move || {
                                let _ = forward_browser.borrow_mut().go_forward(&forward_tab);
                            }),
                    )
                    .child(
                        Button::new("R")
                            .width(34.0)
                            .height(28.0)
                            .appearance(ButtonAppearance::Ghost)
                            .tooltip("Reload")
                            .on_click(move || {
                                let _ = reload_browser.borrow_mut().reload(&reload_tab);
                            }),
                    )
                    .child(Badge::new(protocol))
                    .child(Flex::new().grow(1.0).shrink(1.0).child(
                        AddressField(TextInput::new(address_text).width(address_width).on_input(
                            move |value| {
                                *address_draft.borrow_mut() = Some(value.to_owned());
                            },
                        )),
                    ))
                    .child(
                        Button::new("−")
                            .width(30.0)
                            .height(28.0)
                            .appearance(ButtonAppearance::Ghost)
                            .tooltip("Zoom out")
                            .on_click(move || {
                                let next = (zoom - 0.1).max(0.1);
                                let _ = zoom_out_browser.borrow_mut().set_zoom(&zoom_out_tab, next);
                            }),
                    )
                    .child(Text::new(format!("{:.0}%", state.zoom * 100.0)).size(11.0))
                    .child(
                        Button::new("+")
                            .width(30.0)
                            .height(28.0)
                            .appearance(ButtonAppearance::Ghost)
                            .tooltip("Zoom in")
                            .on_click(move || {
                                let next = (zoom + 0.1).min(10.0);
                                let _ = zoom_in_browser.borrow_mut().set_zoom(&zoom_in_tab, next);
                            }),
                    ),
            ),
        );

    let view = Pad::all(SHELL_INSET).child(
        Column::new()
            .gap(10.0)
            .child(
                Row::new()
                    .width(chrome_width)
                    .align(Align::Center)
                    .gap(12.0)
                    .child(SignalDot)
                    .child(
                        Flex::new().grow(1.0).shrink(1.0).child(
                            Column::new()
                                .gap(1.0)
                                .child(Text::new("SERVO / OFFSCREEN COMPOSITOR").size(9.0))
                                .child(Text::new(page_title).size(22.0).ellipsis()),
                        ),
                    )
                    .child(Text::new("FRAME SYNC").size(9.0))
                    .child(Badge::new("ENGINE 0.4")),
            )
            .child(toolbar)
            .child(
                Stack::new()
                    .size(chrome_width, surface_height + 4.0)
                    .child(ChromeSurface::new(
                        chrome_width,
                        surface_height + 4.0,
                        PAPER,
                        SIGNAL,
                        3.0,
                        2.0,
                    ))
                    .child(
                        Pad::all(2.0).child(
                            Scroll::new()
                                .label(SURFACE_NAME)
                                .size(surface_width, surface_height)
                                .child(
                                    feed.image()
                                        .alt("Web page rendered by Servo")
                                        .size(surface_width, surface_height),
                                ),
                        ),
                    ),
            ),
    );
    let mut app = App::mount_with_theme_size_scaled(
        SERVO_THEME,
        view,
        viewport.width.round().max(1.0) as u32,
        viewport.height.round().max(1.0) as u32,
        scale,
    );
    app.set_clear_color(DEEP_INK);
    app.set_continuous_redraw(continuous);

    let address_browser = browser.clone();
    let address_tab = tab_id.clone();
    app.register_focused_input_handler(Role::TextInput, Some(ADDRESS_NAME), move |event| {
        let FocusedInputEvent::Key(key) = event else {
            return FocusedInputResult::Ignored;
        };
        if key.state != RawInputState::Pressed || key.repeat {
            return FocusedInputResult::Ignored;
        }
        match key.logical_key {
            Key::Named(NamedKey::Enter) => {
                let typed = address.draft.borrow_mut().take();
                let Some(url) = typed.as_deref().and_then(parse_address) else {
                    // Nothing typed, or nothing loadable: show the URL again.
                    address.reset.set(true);
                    return FocusedInputResult::Handled;
                };
                let _ = address_browser.borrow_mut().navigate(&address_tab, url);
                address.reset.set(true);
                FocusedInputResult::Handled
            }
            // Escape abandons an edit. Without one it keeps closing the window.
            Key::Named(NamedKey::Escape) if address.draft.borrow_mut().take().is_some() => {
                address.reset.set(true);
                FocusedInputResult::Handled
            }
            _ => FocusedInputResult::Ignored,
        }
    });

    let input_browser = browser;
    let input_tab = tab_id;
    app.register_focused_input_handler(Role::ScrollView, Some(SURFACE_NAME), move |event| {
        if matches!(
            &event,
            FocusedInputEvent::Key(key) if matches!(key.logical_key, Key::Named(NamedKey::Escape))
        ) {
            return FocusedInputResult::Ignored;
        }
        let Ok(mut input) = BrowserInput::try_from(event) else {
            return FocusedInputResult::Ignored;
        };
        scale_input(&mut input, scale);
        match input_browser.borrow_mut().dispatch_input(&input_tab, input) {
            Ok(()) => FocusedInputResult::Handled,
            Err(_) => FocusedInputResult::Ignored,
        }
    });
    app
}

fn scale_input(input: &mut BrowserInput, scale: f32) {
    let BrowserInput::Pointer(pointer) = input else {
        return;
    };
    pointer.x *= scale;
    pointer.y *= scale;
    if let BrowserPointerKind::Wheel(BrowserWheelDelta::Pixels { x, y }) = &mut pointer.kind {
        *x *= scale;
        *y *= scale;
    }
}

/// Turns typed address-bar text into a URL. Text with a scheme is used as is;
/// bare host names get `https://`, or `http://` for loopback hosts.
fn parse_address(typed: &str) -> Option<Url> {
    let typed = typed.trim();
    if typed.is_empty() || typed.contains(char::is_whitespace) {
        return None;
    }
    if typed.contains("://")
        || ["about:", "data:", "file:"]
            .iter()
            .any(|s| typed.starts_with(s))
    {
        return Url::parse(typed).ok();
    }
    let loopback = ["localhost", "127.0.0.1", "[::1]"]
        .iter()
        .any(|host| typed.starts_with(host));
    let scheme = if loopback { "http" } else { "https" };
    Url::parse(&format!("{scheme}://{typed}"))
        .ok()
        .filter(Url::has_host)
}

/// A [`TextInput`] named for assistive technology without a floating label,
/// which would not fit the toolbar's height. The name also routes Enter and
/// Escape to the address handler.
struct AddressField(TextInput);

impl View for AddressField {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let id = Box::new(self.0).build(ctx, parent);
        ctx.scene.a11y_mut(id).name = Some(ADDRESS_NAME.to_owned());
        id
    }
}

/// A quiet, fixed chrome plate used behind the toolbar and webview. Keeping it
/// example-local makes the visual treatment explicit without adding a fake
/// browser widget to the reusable SchnellUI component library.
struct ChromeSurface {
    size: Size,
    fill: Color,
    outline: Color,
    radius: f32,
    border: f32,
}

impl ChromeSurface {
    fn new(width: f32, height: f32, fill: Color, outline: Color, radius: f32, border: f32) -> Self {
        Self {
            size: Size { width, height },
            fill,
            outline,
            radius,
            border,
        }
    }
}

impl View for ChromeSurface {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        let id = ctx.scene.insert(WidgetKind::Image, parent);
        ctx.scene.a11y_mut(id).role = Role::Group.as_u16();
        let rect = node_rect(ctx.scene, id, this.size);
        let paint = ctx.scene.paint_mut(id);
        paint.primitives.push(Primitive::SolidRect {
            rect,
            color: this.outline,
            corner_radius: this.radius,
        });
        paint.primitives.push(Primitive::SolidRect {
            rect: Rect::new(
                rect.x + this.border,
                rect.y + this.border,
                (rect.width - this.border * 2.0).max(0.0),
                (rect.height - this.border * 2.0).max(0.0),
            ),
            color: this.fill,
            corner_radius: (this.radius - this.border).max(0.0),
        });
        ctx.layout
            .set_measure(id, Box::new(move |_available| this.size));
        id
    }
}

/// The shell's single memorable status mark: a hard orange lamp with an ivory
/// core, exposed as a live accessibility status rather than decorative noise.
struct SignalDot;

impl View for SignalDot {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let size = Size {
            width: 12.0,
            height: 12.0,
        };
        let id = ctx.scene.insert(WidgetKind::Badge, parent);
        let semantics = ctx.scene.a11y_mut(id);
        semantics.role = Role::Status.as_u16();
        semantics.value = Some("Servo frame synchronized".to_owned());
        let rect = node_rect(ctx.scene, id, size);
        let paint = ctx.scene.paint_mut(id);
        paint.primitives.push(Primitive::SolidRect {
            rect,
            color: SIGNAL,
            corner_radius: 6.0,
        });
        paint.primitives.push(Primitive::SolidRect {
            rect: Rect::new(rect.x + 4.0, rect.y + 4.0, 4.0, 4.0),
            color: PAPER,
            corner_radius: 2.0,
        });
        ctx.layout.set_measure(id, Box::new(move |_available| size));
        id
    }
}

struct DemoServer {
    url: Url,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl DemoServer {
    fn start() -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let url = Url::parse(&format!("http://{}/", listener.local_addr()?))
            .map_err(std::io::Error::other)?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let thread = std::thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let mut request = [0; 2048];
                        let _ = stream.read(&mut request);
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
                            DEMO_PAGE.len()
                        );
                        let _ = stream.write_all(response.as_bytes());
                        let _ = stream.write_all(DEMO_PAGE.as_bytes());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            url,
            stop,
            thread: Some(thread),
        })
    }

    fn url(&self) -> Url {
        self.url.clone()
    }
}

impl Drop for DemoServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use schnellui::raw_keyboard::{KeyCode, KeyLocation, PhysicalKey};
    use schnellui::{RawKeyEvent, RawModifiers, UiKey};

    fn press(key: NamedKey, code: KeyCode) -> FocusedInputEvent {
        FocusedInputEvent::Key(RawKeyEvent {
            logical_key: Key::Named(key),
            physical_key: PhysicalKey::Code(code),
            key_without_modifiers: Key::Named(key),
            location: KeyLocation::Standard,
            modifiers: RawModifiers::default(),
            state: RawInputState::Pressed,
            repeat: false,
            text: None,
            text_with_all_modifiers: None,
        })
    }

    #[test]
    fn address_field_navigates_on_enter_and_reverts_on_escape() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let server = DemoServer::start().unwrap();
        let tab_id = BrowserTabId::new(TAB_ID).unwrap();
        let size = Size {
            width: 960.0,
            height: 660.0,
        };
        let (width, height) = browser_physical_size(size, 1.0);
        let engine = ServoEngine::new(width, height).unwrap();
        let mut browser = Browser::restore(engine, BrowserSessionState::default()).unwrap();
        browser
            .open(BrowserTabState::new(tab_id.clone(), server.url()))
            .unwrap();
        let browser = Rc::new(RefCell::new(browser));
        let frame = wait_for_initial_frame(&browser, &tab_id, Duration::from_secs(10)).unwrap();
        let address = AddressBar::default();
        let mut app = browser_app(
            browser.clone(),
            tab_id.clone(),
            FrameFeed::new(frame),
            address.clone(),
            size,
            1.0,
            false,
        );
        let field = app
            .find_widget(Role::TextInput, Some(ADDRESS_NAME))
            .unwrap();
        assert!(app.focus(Some(field)));

        // Escape abandons a typed edit, then falls through to the window.
        app.dispatch_key(UiKey::SelectAll);
        app.dispatch_key(UiKey::Char("abandoned.example"));
        assert_eq!(address.draft.borrow().as_deref(), Some("abandoned.example"));
        let escape = || press(NamedKey::Escape, KeyCode::Escape);
        assert_eq!(
            app.dispatch_focused_input(escape()),
            FocusedInputResult::Handled
        );
        assert_eq!(*address.draft.borrow(), None);
        assert!(address.reset.take());
        assert_eq!(
            app.dispatch_focused_input(escape()),
            FocusedInputResult::Ignored
        );

        // Enter loads the typed address in the tab.
        let host = server.url().host_str().unwrap().to_owned();
        let port = server.url().port().unwrap();
        app.dispatch_key(UiKey::SelectAll);
        app.dispatch_key(UiKey::Char(&format!("{host}:{port}/next")));
        let enter = press(NamedKey::Enter, KeyCode::Enter);
        assert_eq!(
            app.dispatch_focused_input(enter),
            FocusedInputResult::Handled
        );
        assert_eq!(*address.draft.borrow(), None);
        assert!(address.reset.take());
        assert_eq!(
            browser.borrow().tab(&tab_id).unwrap().url.as_str(),
            format!("http://{host}:{port}/next")
        );
    }

    #[test]
    fn address_parsing() {
        let parsed = |typed| parse_address(typed).map(|url| url.to_string());
        assert_eq!(parsed("servo.org").as_deref(), Some("https://servo.org/"));
        assert_eq!(
            parsed(" http://example.com/a?b ").as_deref(),
            Some("http://example.com/a?b")
        );
        assert_eq!(
            parsed("localhost:8000/x").as_deref(),
            Some("http://localhost:8000/x")
        );
        assert_eq!(parsed("about:blank").as_deref(), Some("about:blank"));
        assert_eq!(parsed(""), None);
        assert_eq!(parsed("two words"), None);
    }
}
