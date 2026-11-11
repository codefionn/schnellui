//! `RegisterHotKey` on a thread that owns a message loop.
//!
//! Hot keys registered with no window post `WM_HOTKEY` to the registering
//! thread's queue, so the thread registers, loops on `GetMessageW`, and leaves
//! on `WM_QUIT`. The system reports no key release.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, MOD_NOREPEAT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetMessageW, PeekMessageW, PostThreadMessageW, MSG, PM_NOREMOVE, WM_HOTKEY, WM_QUIT, WM_USER,
};

use super::keys::{display_chord, windows_modifiers, windows_virtual_key};
use super::{GlobalShortcut, GlobalShortcutBinding, GlobalShortcutError, Sink};

pub(super) const SUPPORTED: bool = true;

/// What the handle and the thread share. `thread_id` is 0 until the thread's
/// message queue exists.
struct Shared {
    thread_id: AtomicU32,
    stop: AtomicBool,
}

pub(super) struct Session {
    shared: Arc<Shared>,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        let thread_id = self.shared.thread_id.load(Ordering::SeqCst);
        if thread_id != 0 {
            // SAFETY: posting to a thread id has no memory preconditions; it
            // fails harmlessly when the thread is already gone.
            unsafe { PostThreadMessageW(thread_id, WM_QUIT, 0, 0) };
        }
    }
}

pub(super) fn start(shortcuts: Vec<GlobalShortcut>, sink: Arc<Sink>) -> Session {
    let shared = Arc::new(Shared {
        thread_id: AtomicU32::new(0),
        stop: AtomicBool::new(false),
    });
    let thread_shared = shared.clone();
    let spawned = std::thread::Builder::new()
        .name("schnellui-global-shortcuts".into())
        .spawn({
            let sink = sink.clone();
            move || run(shortcuts, &sink, &thread_shared)
        });
    if let Err(error) = spawned {
        sink.failed(GlobalShortcutError::Platform(error.to_string()));
    }
    Session { shared }
}

fn run(shortcuts: Vec<GlobalShortcut>, sink: &Sink, shared: &Shared) {
    // SAFETY: `msg` is a plain-data struct the calls below fill in; every call
    // gets a valid pointer to it and a null window, which means "this thread's
    // queue".
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        // Any queue call makes the thread's message queue exist, so a quit
        // posted by `Session::drop` after the id is published is not lost.
        PeekMessageW(
            &mut msg,
            std::ptr::null_mut(),
            WM_USER,
            WM_USER,
            PM_NOREMOVE,
        );
        shared
            .thread_id
            .store(GetCurrentThreadId(), Ordering::SeqCst);
        if shared.stop.load(Ordering::SeqCst) {
            return;
        }

        let mut registered: Vec<(i32, String)> = Vec::new();
        let mut bindings = Vec::with_capacity(shortcuts.len());
        for (index, shortcut) in shortcuts.iter().enumerate() {
            let hotkey_id = index as i32 + 1;
            let mut trigger = String::new();
            if let Some(chord) = shortcut.trigger() {
                match windows_virtual_key(chord.logical_key()) {
                    None => sink.failed(GlobalShortcutError::Platform(format!(
                        "{}: no virtual key for the preferred trigger",
                        shortcut.id()
                    ))),
                    Some(key) => {
                        let modifiers = windows_modifiers(chord) | MOD_NOREPEAT;
                        if RegisterHotKey(std::ptr::null_mut(), hotkey_id, modifiers, key) != 0 {
                            registered.push((hotkey_id, shortcut.id().to_owned()));
                            trigger = display_chord(chord, "Ctrl");
                        } else {
                            sink.failed(GlobalShortcutError::Platform(format!(
                                "{}: {} is taken by another program",
                                shortcut.id(),
                                display_chord(chord, "Ctrl")
                            )));
                        }
                    }
                }
            }
            bindings.push(GlobalShortcutBinding {
                id: shortcut.id().to_owned(),
                description: shortcut.description().to_owned(),
                trigger,
            });
        }
        sink.changed(bindings);

        // 0 is WM_QUIT, -1 an error; both end the loop.
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            if msg.message == WM_HOTKEY {
                let hotkey_id = msg.wParam as i32;
                if let Some((_, id)) = registered.iter().find(|(known, _)| *known == hotkey_id) {
                    sink.pressed(id);
                }
            }
        }
        for (hotkey_id, _) in &registered {
            UnregisterHotKey(std::ptr::null_mut(), *hotkey_id);
        }
    }
}
