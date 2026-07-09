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
use schnellui_servo::{Browser, BrowserTabId, BrowserTabState};
use url::Url;

const PAGE: &[u8] = b"<!doctype html><style>html,body{margin:0;width:100%;min-height:100%;background:#0000ff}</style>";

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
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            PAGE.len()
                        );
                        let _ = stream.write_all(response.as_bytes());
                        let _ = stream.write_all(PAGE);
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
fn real_servo_renders_a_new_rgba_frame_after_a_post_load_dom_mutation() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let server = TestServer::start();
    let mut browser = Browser::new(ServoEngine::new(320, 200).unwrap());
    let tab = BrowserTabId::new("mutation").unwrap();
    browser
        .open(BrowserTabState::new(tab.clone(), server.origin.clone()))
        .unwrap();

    let started = Instant::now();
    let initial_frame = loop {
        browser.spin_event_loop();
        if let Some(frame) = browser.render(&tab).unwrap() {
            if browser
                .with_engine_tab(&tab, |engine, handle| engine.is_load_complete(handle))
                .unwrap()
            {
                break frame;
            }
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "Servo should paint the loaded document"
        );
        std::thread::sleep(Duration::from_millis(1));
    };

    let mutation_complete = Rc::new(RefCell::new(false));
    let mutation_result = mutation_complete.clone();
    browser
        .with_engine_tab(&tab, |engine, handle| {
            engine.evaluate_javascript(
                handle,
                "document.documentElement.style.background = '#ff0000'; document.body.style.background = '#ff0000'; 'mutated'",
                move |result| {
                    assert!(result.is_ok(), "DOM mutation should evaluate: {result:?}");
                    *mutation_result.borrow_mut() = true;
                },
            );
        })
        .unwrap();

    let changed_frame = loop {
        browser.spin_event_loop();
        if *mutation_complete.borrow() {
            if let Some(frame) = browser.render(&tab).unwrap() {
                if frame.rgba != initial_frame.rgba {
                    break frame;
                }
            }
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "Servo should notify and render a changed frame after a DOM mutation"
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    assert_ne!(initial_frame.rgba, changed_frame.rgba);
}
