//! JNI glue. Android only. `schnellui::android_app!` exports the native methods
//! of `SchnellBridge` and forwards to the functions here.
//!
//! An app handle is a leaked `Box<AndroidRenderer>`. All calls come from the
//! Android main thread, as the contract requires.

use std::ffi::{c_char, c_int, CString};

use jni::errors::LogErrorAndDefault;
use jni::objects::JValue;
pub use jni::objects::{JByteArray, JClass, JString};
pub use jni::sys::jboolean;
pub use jni::EnvUnowned;
use jni::{jni_sig, jni_str, Env};

use super::{AndroidRenderer, Event, Viewport};

/// The NativeActivity entry point type. `render-android` turns on winit's
/// native-activity backend, whose glue calls `android_main`. Bionic resolves
/// every symbol at load, so `android_app!` defines an empty one.
pub use winit::platform::android::activity::AndroidApp;

/// Builds the app and returns its handle.
pub fn mount<'a>(
    mut env: EnvUnowned<'a>,
    _class: JClass<'a>,
    viewport: Viewport,
    factory: fn(Viewport) -> AndroidRenderer,
) -> i64 {
    env.with_env(|env| -> jni::errors::Result<i64> {
        let vm = env.get_java_vm()?;
        // A thread that Rust attaches finds classes through the system class
        // loader, which does not see the app's classes. Keep the class instead.
        let bridge = env.new_global_ref(&_class)?;
        let ptr = Box::into_raw(Box::new(factory(viewport)));
        let handle = ptr as usize as i64;
        // SAFETY: `ptr` was just leaked and nothing else holds it yet.
        let renderer = unsafe { &mut *ptr };
        // The renderer registers a signal change listener while it lives, so
        // this callback runs on whichever thread wrote the signal.
        renderer.set_wake(move || request_update(&vm, &bridge, handle));
        Ok(handle)
    })
    .resolve::<LogErrorAndDefault>()
}

#[link(name = "log")]
unsafe extern "C" {
    fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
}

const ANDROID_LOG_ERROR: c_int = 6;

/// Writes one line to logcat under the tag `schnellui`.
fn log_error(message: &str) {
    let text = CString::new(message.replace('\0', " ")).unwrap_or_default();
    // SAFETY: both strings are NUL terminated and outlive the call.
    unsafe {
        __android_log_write(ANDROID_LOG_ERROR, c"schnellui".as_ptr(), text.as_ptr());
    }
}

/// Calls the static `SchnellBridge.requestUpdate(long)` from the current
/// thread, attaching it if needed. The host posts the update to its main
/// thread.
///
/// This runs on the thread that wrote a signal, which can be any Rust thread.
/// The attachment is scoped: the thread is attached for this call and detached
/// when it returns, so a worker thread is never left attached to the JVM and
/// can exit without waiting on it. A wake fires at most once per render, so
/// the cost of attaching each time is small. A thread that was already
/// attached stays attached.
///
/// If `requestUpdate` throws, the pending Java exception is cleared so it does
/// not surface in unrelated Java code on this thread, and the error goes to
/// logcat.
fn request_update(vm: &jni::JavaVM, bridge: &jni::refs::Global<JClass<'static>>, handle: i64) {
    let called = vm.attach_current_thread_for_scope(|env| -> jni::errors::Result<()> {
        let result = env.call_static_method(
            bridge,
            jni_str!("requestUpdate"),
            jni_sig!("(J)V"),
            &[JValue::Long(handle)],
        );
        if env.exception_check() {
            env.exception_clear();
        }
        result.map(|_| ())
    });
    if let Err(error) = called {
        log_error(&format!("requestUpdate failed: {error}"));
    }
}

/// # Safety
/// `app` must come from [`mount`] and must not have been unmounted.
unsafe fn app<'a>(handle: i64) -> Option<&'a mut AndroidRenderer> {
    (handle != 0).then(|| unsafe { &mut *(handle as usize as *mut AndroidRenderer) })
}

fn to_array<'a>(env: &mut Env<'a>, bytes: Option<Vec<u8>>) -> jni::errors::Result<JByteArray<'a>> {
    match bytes {
        Some(bytes) => env.byte_array_from_slice(&bytes),
        None => Ok(JByteArray::null()),
    }
}

/// The whole tree, from nothing.
pub fn render<'a>(mut env: EnvUnowned<'a>, _class: JClass<'a>, handle: i64) -> JByteArray<'a> {
    env.with_env(|env| -> jni::errors::Result<JByteArray<'a>> {
        // SAFETY: the host passes the handle `nativeMount` returned.
        let bytes = unsafe { app(handle) }.map(|app| app.render());
        to_array(env, bytes)
    })
    .resolve::<LogErrorAndDefault>()
}

/// One view event. Returns the diff, or null.
#[allow(clippy::too_many_arguments)]
pub fn dispatch<'a>(
    mut env: EnvUnowned<'a>,
    _class: JClass<'a>,
    handle: i64,
    node: i32,
    event: i32,
    i: i32,
    f: f32,
    s: JString<'a>,
) -> JByteArray<'a> {
    env.with_env(|env| -> jni::errors::Result<JByteArray<'a>> {
        let text = if s.is_null() {
            None
        } else {
            Some(s.try_to_string(env)?)
        };
        let bytes = Event::from_raw(event, i, f, text).and_then(|event| {
            // SAFETY: the host passes the handle `nativeMount` returned.
            unsafe { app(handle) }.and_then(|app| app.dispatch(node as u32, event))
        });
        to_array(env, bytes)
    })
    .resolve::<LogErrorAndDefault>()
}

/// A new viewport size. Returns the diff, or null.
pub fn resize<'a>(
    mut env: EnvUnowned<'a>,
    _class: JClass<'a>,
    handle: i64,
    width: f32,
    height: f32,
) -> JByteArray<'a> {
    env.with_env(|env| -> jni::errors::Result<JByteArray<'a>> {
        // SAFETY: the host passes the handle `nativeMount` returned.
        let bytes = unsafe { app(handle) }.and_then(|app| app.resize(width, height));
        to_array(env, bytes)
    })
    .resolve::<LogErrorAndDefault>()
}

/// Renders again after a signal changed outside the host's calls. Returns the
/// diff, or null.
pub fn update<'a>(mut env: EnvUnowned<'a>, _class: JClass<'a>, handle: i64) -> JByteArray<'a> {
    env.with_env(|env| -> jni::errors::Result<JByteArray<'a>> {
        // SAFETY: the host passes the handle `nativeMount` returned.
        let bytes = unsafe { app(handle) }.and_then(|app| app.update());
        to_array(env, bytes)
    })
    .resolve::<LogErrorAndDefault>()
}

/// Drops the app. The handle is dead afterwards.
pub fn unmount<'a>(mut env: EnvUnowned<'a>, _class: JClass<'a>, handle: i64) {
    env.with_env(|_env| -> jni::errors::Result<()> {
        if handle != 0 {
            // SAFETY: the handle came from `mount` and the host unmounts once.
            drop(unsafe { Box::from_raw(handle as usize as *mut AndroidRenderer) });
        }
        Ok(())
    })
    .resolve::<LogErrorAndDefault>()
}
