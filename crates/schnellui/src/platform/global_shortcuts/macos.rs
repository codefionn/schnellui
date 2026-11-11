//! Carbon `RegisterEventHotKey`, called straight from the main thread.
//!
//! Carbon delivers hot key events through the application event target, which
//! the main thread's run loop (winit's) already services. The handler box is
//! owned by the session and freed only after the handler is removed. This
//! file is type-checked from other hosts but has not run on a Mac.

use std::ffi::c_void;
use std::sync::Arc;

use super::keys::{carbon_key_code, carbon_modifiers, display_chord};
use super::{GlobalShortcut, GlobalShortcutBinding, GlobalShortcutError, Sink};

pub(super) const SUPPORTED: bool = true;

type OsStatus = i32;
type EventRef = *mut c_void;
type EventHandlerCallRef = *mut c_void;
type EventTargetRef = *mut c_void;
type EventHandlerRef = *mut c_void;
type EventHotKeyRef = *mut c_void;
type EventHandlerProc = extern "C" fn(EventHandlerCallRef, EventRef, *mut c_void) -> OsStatus;

#[repr(C)]
struct EventTypeSpec {
    event_class: u32,
    event_kind: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct EventHotKeyId {
    signature: u32,
    id: u32,
}

/// `'keyb'`.
const EVENT_CLASS_KEYBOARD: u32 = 0x6B65_7962;
const EVENT_HOT_KEY_PRESSED: u32 = 5;
const EVENT_HOT_KEY_RELEASED: u32 = 6;
/// `'----'`.
const EVENT_PARAM_DIRECT_OBJECT: u32 = 0x2D2D_2D2D;
/// `'hkid'`.
const TYPE_EVENT_HOT_KEY_ID: u32 = 0x686B_6964;
/// `'SCHN'`, tells our hot keys from another library's in the same process.
const SIGNATURE: u32 = 0x5343_484E;
const EVENT_NOT_HANDLED: OsStatus = -9874;

// `ItemCount` and `ByteCount` are declared as `c_ulong`: the callee reads
// either 32 or 64 bits, and a zero-extended 64-bit value is right for both.
#[link(name = "Carbon", kind = "framework")]
extern "C" {
    fn GetApplicationEventTarget() -> EventTargetRef;
    fn InstallEventHandler(
        target: EventTargetRef,
        handler: EventHandlerProc,
        num_types: std::ffi::c_ulong,
        list: *const EventTypeSpec,
        user_data: *mut c_void,
        out_ref: *mut EventHandlerRef,
    ) -> OsStatus;
    fn RemoveEventHandler(handler: EventHandlerRef) -> OsStatus;
    fn RegisterEventHotKey(
        key_code: u32,
        modifiers: u32,
        hot_key_id: EventHotKeyId,
        target: EventTargetRef,
        options: u32,
        out_ref: *mut EventHotKeyRef,
    ) -> OsStatus;
    fn UnregisterEventHotKey(hot_key: EventHotKeyRef) -> OsStatus;
    fn GetEventKind(event: EventRef) -> u32;
    fn GetEventParameter(
        event: EventRef,
        name: u32,
        desired_type: u32,
        actual_type: *mut u32,
        buffer_size: std::ffi::c_ulong,
        actual_size: *mut std::ffi::c_ulong,
        data: *mut c_void,
    ) -> OsStatus;
    fn pthread_main_np() -> i32;
}

/// What the event handler reads: the sink and the id of each hot key, indexed
/// by the Carbon hot key id minus one.
struct Handler {
    sink: Arc<Sink>,
    ids: Vec<String>,
}

pub(super) struct Session {
    handler: Option<Box<Handler>>,
    handler_ref: EventHandlerRef,
    hot_keys: Vec<EventHotKeyRef>,
}

// SAFETY: the raw refs are only touched in `start` and `drop`, which run on the
// main thread (checked in `start`, and `drop` leaks instead of freeing when it
// is elsewhere).
unsafe impl Send for Session {}

extern "C" fn on_hot_key(
    _call: EventHandlerCallRef,
    event: EventRef,
    user: *mut c_void,
) -> OsStatus {
    // SAFETY: `user` is the `Handler` box the session owns; it outlives the
    // installed handler, and the handler only runs on the main thread.
    let handler = unsafe { &*(user as *const Handler) };
    let mut hot_key = EventHotKeyId {
        signature: 0,
        id: 0,
    };
    // SAFETY: `event` is a live hot key event, and the buffer is the size of
    // the type asked for.
    let (kind, status) = unsafe {
        (
            GetEventKind(event),
            GetEventParameter(
                event,
                EVENT_PARAM_DIRECT_OBJECT,
                TYPE_EVENT_HOT_KEY_ID,
                std::ptr::null_mut(),
                std::mem::size_of::<EventHotKeyId>() as std::ffi::c_ulong,
                std::ptr::null_mut(),
                (&mut hot_key as *mut EventHotKeyId).cast(),
            ),
        )
    };
    if status != 0 || hot_key.signature != SIGNATURE {
        return EVENT_NOT_HANDLED;
    }
    let Some(id) = (hot_key.id as usize)
        .checked_sub(1)
        .and_then(|index| handler.ids.get(index))
    else {
        return EVENT_NOT_HANDLED;
    };
    match kind {
        EVENT_HOT_KEY_PRESSED => handler.sink.pressed(id),
        EVENT_HOT_KEY_RELEASED => handler.sink.released(id),
        _ => return EVENT_NOT_HANDLED,
    }
    0
}

pub(super) fn start(shortcuts: Vec<GlobalShortcut>, sink: Arc<Sink>) -> Session {
    let mut session = Session {
        handler: None,
        handler_ref: std::ptr::null_mut(),
        hot_keys: Vec::new(),
    };
    // SAFETY: plain query with no arguments.
    if unsafe { pthread_main_np() } != 1 {
        sink.failed(GlobalShortcutError::Platform(
            "global shortcuts must be registered on the main thread".into(),
        ));
        return session;
    }
    let handler = Box::new(Handler {
        sink: sink.clone(),
        ids: shortcuts.iter().map(|s| s.id().to_owned()).collect(),
    });
    let spec = [
        EventTypeSpec {
            event_class: EVENT_CLASS_KEYBOARD,
            event_kind: EVENT_HOT_KEY_PRESSED,
        },
        EventTypeSpec {
            event_class: EVENT_CLASS_KEYBOARD,
            event_kind: EVENT_HOT_KEY_RELEASED,
        },
    ];
    // SAFETY: `spec` is valid for the call, and the handler box is stored in
    // the session, which removes the handler before the box is freed.
    let status = unsafe {
        let target = GetApplicationEventTarget();
        InstallEventHandler(
            target,
            on_hot_key,
            spec.len() as std::ffi::c_ulong,
            spec.as_ptr(),
            (&*handler as *const Handler).cast_mut().cast(),
            &mut session.handler_ref,
        )
    };
    session.handler = Some(handler);
    if status != 0 {
        session.handler_ref = std::ptr::null_mut();
        sink.failed(GlobalShortcutError::Platform(format!(
            "InstallEventHandler failed with status {status}"
        )));
        return session;
    }

    let mut bindings = Vec::with_capacity(shortcuts.len());
    for (index, shortcut) in shortcuts.iter().enumerate() {
        let mut trigger = String::new();
        if let Some(chord) = shortcut.trigger() {
            match carbon_key_code(chord.logical_key()) {
                None => sink.failed(GlobalShortcutError::Platform(format!(
                    "{}: no hot key code for the preferred trigger",
                    shortcut.id()
                ))),
                Some(code) => {
                    let mut hot_key: EventHotKeyRef = std::ptr::null_mut();
                    // SAFETY: valid out pointer; the target is the application's.
                    let status = unsafe {
                        RegisterEventHotKey(
                            code,
                            carbon_modifiers(chord),
                            EventHotKeyId {
                                signature: SIGNATURE,
                                id: index as u32 + 1,
                            },
                            GetApplicationEventTarget(),
                            0,
                            &mut hot_key,
                        )
                    };
                    if status == 0 && !hot_key.is_null() {
                        session.hot_keys.push(hot_key);
                        trigger = display_chord(chord, "Cmd");
                    } else {
                        sink.failed(GlobalShortcutError::Platform(format!(
                            "{}: RegisterEventHotKey failed with status {status}",
                            shortcut.id()
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
    session
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: each ref came from the call that made it and is released once.
        unsafe {
            for hot_key in self.hot_keys.drain(..) {
                UnregisterEventHotKey(hot_key);
            }
            if !self.handler_ref.is_null() {
                RemoveEventHandler(self.handler_ref);
            }
            // Off the main thread the handler could be running now. Leaking
            // the small box is the sound choice there.
            if pthread_main_np() != 1 {
                std::mem::forget(self.handler.take());
            }
        }
    }
}
