package io.github.codefionn.schnellui;

/**
 * The JNI surface of the native app. All calls happen on the Android main thread.
 * The signatures are fixed by docs/android-renderer.md.
 */
public final class SchnellBridge {
    private SchnellBridge() {}

    static native long nativeMount(float widthDp, float heightDp, float density, boolean dark);

    static native byte[] nativeRender(long app);

    static native byte[] nativeDispatch(
            long app, int node, int event, int i, float f, String s);

    static native byte[] nativeResize(long app, float widthDp, float heightDp);

    static native void nativeUnmount(long app);

    /** Loads the native library by its short name, for example "android_native". */
    static void load(String lib) {
        System.loadLibrary(lib);
    }

    /** An event sink bound to one mounted app. */
    static EventGate.Sink sinkFor(final long app) {
        return (node, event, i, f, s) -> nativeDispatch(app, node, event, i, f, s);
    }
}
