# schnellui on Android

This is the Java side of the Android renderer. Rust renders a template into a tree, diffs it and sends ops. The host here turns the ops into `android.widget` views. See [docs/android-renderer.md](../docs/android-renderer.md) for the op format and the JNI contract.

## Layout

- `host` is an Android library, package `io.github.codefionn.schnellui`. It has no dependencies. No AndroidX, no Kotlin.
  - `Codes` holds every op, kind, prop and event code.
  - `OpReader` decodes a buffer. It is plain Java and runs in JVM tests.
  - `ViewApplier` applies ops to views and attaches the event listeners.
  - `SchnellBridge` declares the five native methods.
  - `SchnellActivity` loads the library, mounts the app and feeds it resizes.
- `demo` is an application that extends `SchnellActivity`. Its manifest names the native library in the `schnellui.lib` meta-data entry.
- `build-rust.sh` builds `examples/android_native` and copies the `.so` files into `demo/src/main/jniLibs/<abi>/`.

The demo uses the platform theme `Theme.DeviceDefault.DayNight`, so it follows the system dark mode.

## Prerequisites

- JDK 17 or newer. JDK 27 works as the Gradle launcher.
- An Android SDK with platform 37 and build-tools 37.0.0. Set `ANDROID_HOME` or write `sdk.dir=...` to `android/local.properties`.
- For the native library: the Android NDK, `cargo install cargo-ndk` and the Rust targets `aarch64-linux-android` and `x86_64-linux-android`. The script adds the targets.

The Gradle Android plugin comes from Google Maven on the first build.

## Build

```sh
cd android
./gradlew :host:testDebugUnitTest
./gradlew :host:assembleDebug :demo:assembleDebug
```

The demo builds without any `.so`. The APK then lacks the native library, and the app shows a message instead of the UI.

To include the native library:

```sh
./build-rust.sh
./gradlew :demo:assembleDebug
```

The APK is `demo/build/outputs/apk/debug/demo-debug.apk`.

## Install and launch

```sh
adb install -r demo/build/outputs/apk/debug/demo-debug.apk
adb shell am start -n io.github.codefionn.schnellui.demo/.DemoActivity
```

Or run `./gradlew :demo:installDebug` and then the `am start` line.

## How the pieces fit

1. `SchnellActivity` loads the library, calls `nativeMount` with the window size in dp, the density and the night mode, and then `nativeRender`.
2. `ViewApplier` applies the returned buffer. The last op, SET_ROOT, puts the view tree into the activity's container.
3. A click, toggle, slider drag or text edit calls `nativeDispatch`. The returned diff is applied at once.
4. The host sends no event while it applies ops. A SET that already matches the view does nothing, so the cursor and IME state of an `EditText` survive.
5. A size change calls `nativeResize`. A night mode change unmounts and mounts again.

An app activity needs `android:configChanges` for `orientation|screenSize|smallestScreenSize|screenLayout|density|uiMode`. Without it Android recreates the activity on rotation.

## Layout notes

Android does the layout. Some props map approximately:

- MAX_WIDTH and MAX_HEIGHT only clamp an explicit WIDTH, HEIGHT or BASIS. Platform views have no max size.
- JUSTIFY space-between, space-around and space-evenly are done in `FlexLinearLayout` after the normal layout pass.
- WRAP is ignored.
- A column header role is the only heading role, because the Rust `Role` table has no Heading.
