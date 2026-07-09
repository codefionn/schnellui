use std::collections::BTreeMap;
use std::convert::Infallible;
use std::time::Duration;

use schnellui::widgets::CursorIcon;
use schnellui::{
    scene::Point, FocusedInputEvent, RawInputState, RawModifiers, RawPointerAction,
    RawPointerButton, RawPointerEvent,
};
use schnellui_servo::{
    Browser, BrowserEngine, BrowserInput, BrowserMouseButton, BrowserPointerKind,
    BrowserSessionState, BrowserStateStore, BrowserTabId, BrowserTabState, PersistedCookie,
};
use url::Url;

#[derive(Default)]
struct TestEngine {
    next: u64,
    open: BTreeMap<u64, Url>,
    active: Option<u64>,
    events: Vec<(u64, BrowserInput)>,
    cookies: Vec<PersistedCookie>,
    cursor: CursorIcon,
}

impl BrowserEngine for TestEngine {
    type TabHandle = u64;
    type Error = Infallible;

    fn open_tab(&mut self, state: &BrowserTabState) -> Result<Self::TabHandle, Self::Error> {
        self.next += 1;
        self.open.insert(self.next, state.url.clone());
        Ok(self.next)
    }

    fn close_tab(&mut self, handle: Self::TabHandle) {
        self.open.remove(&handle);
    }

    fn set_active(&mut self, handle: &Self::TabHandle, active: bool) {
        if active {
            self.active = Some(*handle);
        } else if self.active == Some(*handle) {
            self.active = None;
        }
    }

    fn navigate(&mut self, handle: &Self::TabHandle, url: &Url) {
        self.open.insert(*handle, url.clone());
    }

    fn go_back(&mut self, handle: &Self::TabHandle, restored_target: &Url) {
        self.open.insert(*handle, restored_target.clone());
    }

    fn go_forward(&mut self, handle: &Self::TabHandle, restored_target: &Url) {
        self.open.insert(*handle, restored_target.clone());
    }
    fn reload(&mut self, _handle: &Self::TabHandle) {}
    fn set_zoom(&mut self, _handle: &Self::TabHandle, _zoom: f32) {}

    fn dispatch_input(&mut self, handle: &Self::TabHandle, input: BrowserInput) {
        self.events.push((*handle, input));
    }

    fn spin_event_loop(&mut self) {}

    fn cursor(&self, _handle: &Self::TabHandle) -> CursorIcon {
        self.cursor
    }

    fn restore_cookies(&mut self, cookies: &[PersistedCookie]) -> Result<(), Self::Error> {
        self.cookies = cookies.to_vec();
        Ok(())
    }

    fn snapshot_cookies(&mut self, _origins: &[Url]) -> Result<Vec<PersistedCookie>, Self::Error> {
        Ok(self.cookies.clone())
    }
}

fn tab(id: &str, url: &str) -> BrowserTabState {
    BrowserTabState::new(BrowserTabId::new(id).unwrap(), Url::parse(url).unwrap())
}

#[test]
fn tabs_share_one_cookie_session_but_keep_independent_history() {
    let cookie = PersistedCookie {
        origin: Url::parse("https://example.test/").unwrap(),
        set_cookie: "session=shared; Path=/; Secure; HttpOnly".into(),
    };
    let state = BrowserSessionState {
        cookies: vec![cookie.clone()],
        ..BrowserSessionState::default()
    };
    let mut browser = Browser::restore(TestEngine::default(), state).unwrap();
    let one = BrowserTabId::new("one").unwrap();
    let two = BrowserTabId::new("two").unwrap();
    browser
        .open(tab("one", "https://example.test/one"))
        .unwrap();
    browser
        .open(tab("two", "https://example.test/two"))
        .unwrap();
    browser
        .navigate(&one, Url::parse("https://example.test/next").unwrap())
        .unwrap();

    assert_eq!(browser.tab(&one).unwrap().history.len(), 2);
    assert_eq!(browser.tab(&two).unwrap().history.len(), 1);
    assert_eq!(browser.active_tab(), Some(&two));
    assert_eq!(browser.snapshot().unwrap().cookies, vec![cookie]);
}

#[test]
fn browser_exposes_the_active_engine_cursor_without_engine_specific_access() {
    let engine = TestEngine {
        cursor: CursorIcon::Text,
        ..TestEngine::default()
    };
    let mut browser = Browser::new(engine);
    let id = BrowserTabId::new("cursor").unwrap();
    browser
        .open(tab("cursor", "https://example.test/"))
        .unwrap();

    assert_eq!(browser.cursor(&id).unwrap(), CursorIcon::Text);
}

#[test]
fn state_store_round_trips_tabs_history_zoom_scroll_and_cookies() {
    let directory = tempfile::tempdir().unwrap();
    let store = BrowserStateStore::new(directory.path().join("browser.json"));
    let mut first = tab("docs", "https://example.test/docs");
    first.zoom = 1.25;
    first.scroll_x = 12.0;
    first.scroll_y = 480.0;
    first.content_height = 2_400.0;
    first.viewport_height = 720.0;
    let state = BrowserSessionState {
        active_tab: Some(first.id.clone()),
        tabs: vec![first],
        cookies: vec![PersistedCookie {
            origin: Url::parse("https://example.test/").unwrap(),
            set_cookie: "theme=dark; Path=/; SameSite=Lax".into(),
        }],
        ..BrowserSessionState::default()
    };

    store.save(&state).unwrap();
    assert_eq!(store.load().unwrap(), state);
}

#[test]
fn restored_history_uses_saved_urls_when_the_new_webview_has_no_native_history() {
    let mut restored_tab = tab("restored", "https://example.test/current");
    restored_tab.history = vec![
        Url::parse("https://example.test/first").unwrap(),
        restored_tab.url.clone(),
    ];
    restored_tab.history_index = 1;
    let id = restored_tab.id.clone();
    let mut browser = Browser::restore(
        TestEngine::default(),
        BrowserSessionState {
            active_tab: Some(id.clone()),
            tabs: vec![restored_tab],
            ..BrowserSessionState::default()
        },
    )
    .unwrap();

    browser.go_back(&id).unwrap();
    let after_back = browser
        .with_engine_tab(&id, |engine, handle| engine.open[handle].clone())
        .unwrap();
    assert_eq!(after_back.as_str(), "https://example.test/first");

    browser.go_forward(&id).unwrap();
    let after_forward = browser
        .with_engine_tab(&id, |engine, handle| engine.open[handle].clone())
        .unwrap();
    assert_eq!(after_forward.as_str(), "https://example.test/current");
}

#[test]
fn schnellui_mouse_buttons_and_coordinates_reach_the_page_losslessly() {
    let input = BrowserInput::try_from(FocusedInputEvent::Pointer(RawPointerEvent {
        position: Point { x: 42.5, y: 17.25 },
        window_position: Point {
            x: 142.5,
            y: 217.25,
        },
        modifiers: RawModifiers {
            shift: true,
            ..RawModifiers::default()
        },
        action: RawPointerAction::Button {
            button: RawPointerButton::Forward,
            state: RawInputState::Pressed,
        },
    }))
    .unwrap();

    let BrowserInput::Pointer(pointer) = input else {
        panic!("pointer event expected");
    };
    assert_eq!((pointer.x, pointer.y), (42.5, 17.25));
    assert!(pointer.modifiers.shift);
    assert_eq!(
        pointer.kind,
        BrowserPointerKind::Button {
            button: BrowserMouseButton::Forward,
            pressed: true,
        }
    );
}

#[test]
fn controller_input_path_stays_well_below_one_frame_budget() {
    let mut browser = Browser::new(TestEngine::default());
    let id = BrowserTabId::new("perf").unwrap();
    browser
        .open(tab("perf", "https://example.test/perf"))
        .unwrap();
    let event = BrowserInput::Pointer(schnellui_servo::BrowserPointerEvent {
        x: 10.0,
        y: 20.0,
        modifiers: Default::default(),
        kind: BrowserPointerKind::Move,
    });

    for _ in 0..20_000 {
        browser.dispatch_input(&id, event.clone()).unwrap();
    }
    let metrics = browser.metrics();
    assert_eq!(metrics.input_samples, 20_000);
    assert!(
        metrics.mean_input_latency() < Duration::from_micros(100),
        "mean controller latency was {:?}",
        metrics.mean_input_latency()
    );
    assert!(
        metrics.input_max < Duration::from_millis(16),
        "one controller dispatch exceeded a 60 Hz frame: {:?}",
        metrics.input_max
    );
}
