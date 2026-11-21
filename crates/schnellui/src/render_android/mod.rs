//! Native Android renderer.
//!
//! A template becomes real `android.widget` views. Rust renders a node tree,
//! diffs it against the previous one and encodes the difference as an op
//! buffer. A Java host applies the buffer in one JNI call per update. The byte
//! layout, codes and JNI signatures are in `docs/android-renderer.md`.
//!
//! The tree, the diff, the op codec and [`AndroidRenderer`] build on every
//! target. Only the JNI glue is `cfg(target_os = "android")`. Android does the
//! layout, so there is no taffy, no wgpu and no retained scene here. SOUL's
//! zero-allocation rule does not cover this renderer.

mod diff;
#[cfg(target_os = "android")]
pub mod glue;
pub mod ops;
mod renderer;
mod template;
#[cfg(test)]
mod tests;
mod tree;

pub use ops::{decode, encode, DecodeError, Op};
pub use renderer::{AndroidRenderer, Event, Viewport};
pub use template::AndroidTemplate;
pub use tree::AndroidNode;

/// Exports the `SchnellBridge` native methods from a `cdylib`.
///
/// ```ignore
/// schnellui::android_app!(|| view());
/// ```
///
/// The argument is evaluated once per mount and must be an `FnMut() -> impl
/// Template`. The closure runs once per render. To keep signals across
/// renders, create them in a block that returns the closure:
/// `android_app!({ let state = State::new(); move || view(&state) })`.
///
/// Off Android the macro expands to nothing, so the crate still builds and
/// tests on the host.
#[cfg(not(target_os = "android"))]
#[macro_export]
macro_rules! android_app {
    ($view:expr) => {};
}

/// Exports the `SchnellBridge` native methods from a `cdylib`.
///
/// See the other definition of this macro for the argument. The expansion is
/// the five `extern "system"` functions of `docs/android-renderer.md`.
#[cfg(target_os = "android")]
#[macro_export]
macro_rules! android_app {
    ($view:expr) => {
        /// Satisfies winit's NativeActivity glue. The Java host never starts a
        /// NativeActivity, so this does nothing. Without it the library fails
        /// to load with `cannot locate symbol "android_main"`.
        #[unsafe(no_mangle)]
        fn android_main(_app: $crate::render_android::glue::AndroidApp) {}

        fn __schnellui_android_mount(
            viewport: $crate::render_android::Viewport,
        ) -> $crate::render_android::AndroidRenderer {
            $crate::render_android::AndroidRenderer::new(viewport, $view)
        }

        #[unsafe(no_mangle)]
        pub extern "system" fn Java_io_github_codefionn_schnellui_SchnellBridge_nativeMount<'a>(
            env: $crate::render_android::glue::EnvUnowned<'a>,
            class: $crate::render_android::glue::JClass<'a>,
            width: f32,
            height: f32,
            density: f32,
            dark: $crate::render_android::glue::jboolean,
        ) -> i64 {
            $crate::render_android::glue::mount(
                env,
                class,
                $crate::render_android::Viewport::new(width, height, density, dark),
                __schnellui_android_mount,
            )
        }

        #[unsafe(no_mangle)]
        pub extern "system" fn Java_io_github_codefionn_schnellui_SchnellBridge_nativeRender<'a>(
            env: $crate::render_android::glue::EnvUnowned<'a>,
            class: $crate::render_android::glue::JClass<'a>,
            app: i64,
        ) -> $crate::render_android::glue::JByteArray<'a> {
            $crate::render_android::glue::render(env, class, app)
        }

        #[unsafe(no_mangle)]
        pub extern "system" fn Java_io_github_codefionn_schnellui_SchnellBridge_nativeDispatch<
            'a,
        >(
            env: $crate::render_android::glue::EnvUnowned<'a>,
            class: $crate::render_android::glue::JClass<'a>,
            app: i64,
            node: i32,
            event: i32,
            i: i32,
            f: f32,
            s: $crate::render_android::glue::JString<'a>,
        ) -> $crate::render_android::glue::JByteArray<'a> {
            $crate::render_android::glue::dispatch(env, class, app, node, event, i, f, s)
        }

        #[unsafe(no_mangle)]
        pub extern "system" fn Java_io_github_codefionn_schnellui_SchnellBridge_nativeResize<'a>(
            env: $crate::render_android::glue::EnvUnowned<'a>,
            class: $crate::render_android::glue::JClass<'a>,
            app: i64,
            width: f32,
            height: f32,
        ) -> $crate::render_android::glue::JByteArray<'a> {
            $crate::render_android::glue::resize(env, class, app, width, height)
        }

        #[unsafe(no_mangle)]
        pub extern "system" fn Java_io_github_codefionn_schnellui_SchnellBridge_nativeUnmount<
            'a,
        >(
            env: $crate::render_android::glue::EnvUnowned<'a>,
            class: $crate::render_android::glue::JClass<'a>,
            app: i64,
        ) {
            $crate::render_android::glue::unmount(env, class, app)
        }
    };
}
