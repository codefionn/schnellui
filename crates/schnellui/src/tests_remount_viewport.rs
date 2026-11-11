//! A remount under a resized window lays the replacement out at the window's
//! live size, whatever size the hook mounted it at, with or without a GPU
//! surface (the headless host has none).

use super::*;
use crate::windowed::headless::HeadlessHost;
use crate::{Remount, WindowUpdate};
use schnellui::scene::Size;
use schnellui::widgets::Button;
use std::cell::Cell;
use std::rc::Rc;

#[test]
fn a_replacement_mounted_at_the_launch_size_takes_the_live_size() {
    let mut app = App::mount_with_size(Button::new("Save"), 640, 700);
    app.frame();
    let mut host = HeadlessHost::new(app);
    let pending = Rc::new(Cell::new(false));
    let flag = pending.clone();
    host.set_remount(move |_: Size, _| {
        if !flag.replace(false) {
            return None;
        }
        // The hook ignores the viewport and mounts at its launch size.
        let app = App::mount_with_size(Button::new("Save"), 640, 700);
        Some(WindowUpdate::Remount(Remount::new(app, "test")))
    });
    host.resize(360.0, 170.0);
    pending.set(true);
    host.resize(360.0, 170.0);
    assert_eq!(host.remounts(), 1);
    let size = host.app().size();
    assert_eq!((size.width, size.height), (360.0, 170.0));
}

#[test]
fn rebuild_on_resize_builds_only_when_the_size_changes() {
    let mut hook = App::rebuild_on_resize((640, 700), |w, h| {
        App::mount_with_size(Button::new("Save"), w, h)
    });
    assert!(hook(Size {
        width: 640.0,
        height: 700.0
    })
    .is_none());
    let app = hook(Size {
        width: 360.4,
        height: 170.0,
    })
    .expect("a new size rebuilds");
    assert_eq!((app.size().width, app.size().height), (360.0, 170.0));
    assert!(hook(Size {
        width: 360.0,
        height: 170.0
    })
    .is_none());
    assert!(hook(Size {
        width: 640.0,
        height: 700.0
    })
    .is_some());
}
