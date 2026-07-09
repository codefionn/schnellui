#![cfg(feature = "servo-engine")]

use std::cell::RefCell;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use schnellui_servo::servo_engine::ServoEngine;
use schnellui_servo::{
    Browser, BrowserInput, BrowserKeyEvent, BrowserModifiers, BrowserMouseButton,
    BrowserPointerEvent, BrowserPointerKind, BrowserSessionState, BrowserTabId, BrowserTabState,
    BrowserWheelDelta, PersistedCookie,
};
use url::Url;

fn state(id: &str, url: &str) -> BrowserTabState {
    BrowserTabState::new(BrowserTabId::new(id).unwrap(), Url::parse(url).unwrap())
}

struct TestServer {
    origin: Url,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl TestServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let origin = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = stop.clone();
        let thread = std::thread::spawn(move || {
            while !stop_for_thread.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let mut request = [0; 2048];
                        let _ = stream.read(&mut request);
                        let body = b"<!doctype html><style>body{height:1200px}</style><script>window.mouseMoves=0;window.mouseDowns=0;window.wheels=0;window.keys=[];addEventListener('mousemove',()=>mouseMoves++);addEventListener('mousedown',()=>mouseDowns++);addEventListener('wheel',()=>wheels++);addEventListener('keydown',e=>keys.push(e.key))</script><input autofocus>";
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes());
                        let _ = stream.write_all(body);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("test server failed: {error}"),
                }
            }
        });
        Self {
            origin,
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap();
    }
}

#[test]
fn real_servo_shares_cookies_renders_and_accepts_mouse_and_keyboard() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let server = TestServer::start();
    let cookie = PersistedCookie {
        origin: server.origin.clone(),
        set_cookie: "servo_session=shared; Path=/; SameSite=Lax".into(),
    };
    let engine = ServoEngine::new(320, 200).expect("software GL context should initialize");
    let mut browser = Browser::restore(
        engine,
        BrowserSessionState {
            cookies: vec![cookie],
            ..BrowserSessionState::default()
        },
    )
    .unwrap();
    let first = BrowserTabId::new("first").unwrap();
    let second = BrowserTabId::new("second").unwrap();
    browser
        .open(state("first", server.origin.join("one").unwrap().as_str()))
        .unwrap();
    browser
        .open(state("second", server.origin.join("two").unwrap().as_str()))
        .unwrap();
    browser.activate(&first).unwrap();

    let started = Instant::now();
    let mut frame = None;
    let mut first_loaded = false;
    let mut second_loaded = false;
    while (!first_loaded || !second_loaded || frame.is_none())
        && started.elapsed() < Duration::from_secs(10)
    {
        browser.spin_event_loop();
        frame = browser.render(&first).unwrap().or(frame);
        first_loaded = browser
            .with_engine_tab(&first, |engine, tab| engine.is_load_complete(tab))
            .unwrap();
        second_loaded = browser
            .with_engine_tab(&second, |engine, tab| engine.is_load_complete(tab))
            .unwrap();
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(first_loaded && second_loaded, "Servo should load both tabs");
    let frame = frame.expect("Servo should paint an offscreen frame within ten seconds");
    assert_eq!((frame.width, frame.height), (320, 200));
    assert_eq!(frame.rgba.len(), 320 * 200 * 4);
    assert!(started.elapsed() < Duration::from_secs(10));

    browser.activate(&first).unwrap();
    let focus_started = Instant::now();
    loop {
        browser.spin_event_loop();
        if browser
            .with_engine_tab(&first, |engine, tab| engine.is_focused(tab))
            .unwrap()
        {
            break;
        }
        assert!(focus_started.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(1));
    }
    let _ = browser.render(&first).unwrap();
    browser
        .dispatch_input(
            &first,
            BrowserInput::Pointer(BrowserPointerEvent {
                x: 24.0,
                y: 32.0,
                modifiers: BrowserModifiers::default(),
                kind: BrowserPointerKind::Move,
            }),
        )
        .unwrap();
    for pressed in [true, false] {
        browser
            .dispatch_input(
                &first,
                BrowserInput::Pointer(BrowserPointerEvent {
                    x: 24.0,
                    y: 32.0,
                    modifiers: BrowserModifiers::default(),
                    kind: BrowserPointerKind::Button {
                        button: BrowserMouseButton::Left,
                        pressed,
                    },
                }),
            )
            .unwrap();
    }
    browser
        .dispatch_input(
            &first,
            BrowserInput::Pointer(BrowserPointerEvent {
                x: 24.0,
                y: 32.0,
                modifiers: BrowserModifiers::default(),
                kind: BrowserPointerKind::Wheel(BrowserWheelDelta::Lines { x: 0.0, y: -1.0 }),
            }),
        )
        .unwrap();
    browser
        .dispatch_input(
            &first,
            BrowserInput::Key(BrowserKeyEvent {
                key: "a".into(),
                code: "KeyA".into(),
                pressed: true,
                repeat: false,
                modifiers: BrowserModifiers::default(),
                text: Some("a".into()),
            }),
        )
        .unwrap();
    for _ in 0..10 {
        browser.spin_event_loop();
        let _ = browser.render(&first).unwrap();
        std::thread::sleep(Duration::from_millis(1));
    }
    let evaluation = Rc::new(RefCell::new(None));
    let result = evaluation.clone();
    browser
        .with_engine_tab(&first, |engine, tab| {
            engine.evaluate_javascript(
                tab,
                "[window.mouseMoves, window.mouseDowns, window.wheels, window.keys]",
                move |value| {
                    *result.borrow_mut() = Some(value);
                },
            );
        })
        .unwrap();
    let evaluation_started = Instant::now();
    while evaluation.borrow().is_none() && evaluation_started.elapsed() < Duration::from_secs(5) {
        browser.spin_event_loop();
        std::thread::sleep(Duration::from_millis(1));
    }
    let debug_value = format!("{:?}", evaluation.borrow().as_ref().unwrap());
    assert!(
        debug_value.contains("Number(1"),
        "mouse event missing: {debug_value}"
    );
    assert!(
        debug_value.contains("Number(1.0), Number(1.0), Number(1.0)"),
        "mouse button or wheel event missing: {debug_value}"
    );
    assert!(
        debug_value.contains("String(\"a\")"),
        "key event missing: {debug_value}"
    );

    let cookie_evaluation = Rc::new(RefCell::new(None));
    let cookie_result = cookie_evaluation.clone();
    browser
        .with_engine_tab(&second, |engine, tab| {
            engine.evaluate_javascript(tab, "document.cookie", move |value| {
                *cookie_result.borrow_mut() = Some(value);
            });
        })
        .unwrap();
    let cookie_started = Instant::now();
    while cookie_evaluation.borrow().is_none() && cookie_started.elapsed() < Duration::from_secs(5)
    {
        browser.spin_event_loop();
        std::thread::sleep(Duration::from_millis(1));
    }
    let cookie_value = format!("{:?}", cookie_evaluation.borrow().as_ref().unwrap());
    assert!(
        cookie_value.contains("servo_session=shared"),
        "second tab could not see the shared cookie: {cookie_value}"
    );

    let scroll_started = Instant::now();
    let snapshot = loop {
        browser.spin_event_loop();
        let snapshot = browser.snapshot().unwrap();
        if snapshot.tabs[0].scroll_y > 0.0
            && snapshot.tabs[0].content_height >= 1_200.0
            && snapshot.tabs[0].viewport_height == 200.0
        {
            break snapshot;
        }
        assert!(scroll_started.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(1));
    };
    assert_eq!(snapshot.tabs.len(), 2);
    assert_eq!(snapshot.active_tab, Some(first));
    assert!(snapshot.tabs[0].scroll_y > 0.0);
    assert!(snapshot.tabs[0].content_height >= 1_200.0);
    assert_eq!(snapshot.tabs[0].viewport_height, 200.0);
    assert!(snapshot.cookies.iter().any(|cookie| {
        cookie.origin.host_str() == Some("127.0.0.1")
            && cookie.set_cookie.contains("servo_session=shared")
    }));
    let metrics = browser.metrics();
    assert_eq!(metrics.input_samples, 5);
    assert!(metrics.input_max < Duration::from_millis(16));
    assert!(metrics.render_max < Duration::from_millis(250));
    assert!(metrics.spin_max < Duration::from_millis(250));
}
