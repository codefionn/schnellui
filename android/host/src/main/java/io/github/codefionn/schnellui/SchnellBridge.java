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

    static native byte[] nativeUpdate(long app);

    private static UpdateScheduler scheduler;

    private static synchronized UpdateScheduler scheduler() {
        if (scheduler == null) {
            final android.os.Handler main = new android.os.Handler(android.os.Looper.getMainLooper());
            scheduler = new UpdateScheduler(main::post);
        }
        return scheduler;
    }

    /**
     * Called by Rust, from any thread, when a signal changes outside a dispatch. Posts one
     * main-thread task per app that calls nativeUpdate. Dropped for an app with no attached
     * activity. Kept by the consumer rules in the host module.
     */
    static void requestUpdate(long app) {
        scheduler().request(app);
    }

    /** Lets requestUpdate reach the applier of the app. Call on the main thread. */
    static void register(final long app, final ViewApplier applier) {
        scheduler().register(app, a -> applier.apply(nativeUpdate(a)));
    }

    static void unregister(long app) {
        scheduler().unregister(app);
    }

    /** Loads the native library by its short name, for example "android_native". */
    static void load(String lib) {
        System.loadLibrary(lib);
    }

    /** An event sink bound to one mounted app. */
    static EventGate.Sink sinkFor(final long app) {
        return (node, event, i, f, s) -> nativeDispatch(app, node, event, i, f, s);
    }
}
