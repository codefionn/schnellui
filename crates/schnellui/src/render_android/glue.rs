//! JNI glue. Android only. `schnellui::android_app!` exports the native methods
//! of `SchnellBridge` and forwards to the functions here.
//!
//! An app handle is a leaked `Box<AndroidRenderer>`. All calls come from the
//! Android main thread, as the contract requires.

use jni::errors::LogErrorAndDefault;
pub use jni::objects::{JByteArray, JClass, JString};
pub use jni::sys::jboolean;
use jni::Env;
pub use jni::EnvUnowned;

use super::{AndroidRenderer, Event, Viewport};

/// Builds the app and returns its handle.
pub fn mount<'a>(
    mut env: EnvUnowned<'a>,
    _class: JClass<'a>,
    viewport: Viewport,
    factory: fn(Viewport) -> AndroidRenderer,
) -> i64 {
    env.with_env(|_env| -> jni::errors::Result<i64> {
        Ok(Box::into_raw(Box::new(factory(viewport))) as usize as i64)
    })
    .resolve::<LogErrorAndDefault>()
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
