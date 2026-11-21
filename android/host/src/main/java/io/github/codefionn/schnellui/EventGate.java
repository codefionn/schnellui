package io.github.codefionn.schnellui;

/**
 * Sends view events to the native side, except while the host applies ops itself.
 * This is what keeps a SET from echoing back as a user event.
 */
public final class EventGate {
    /** Where events go. SchnellBridge is the production sink. */
    public interface Sink {
        byte[] dispatch(int node, int event, int i, float f, String s);
    }

    private final Sink sink;
    private int applying;

    public EventGate(Sink sink) {
        this.sink = sink;
    }

    /** Marks the start of an op application. Calls nest. */
    public void begin() {
        applying++;
    }

    public void end() {
        applying--;
    }

    public boolean isApplying() {
        return applying > 0;
    }

    /** Returns the diff for the event, or null when suppressed or nothing changed. */
    public byte[] fire(int node, int event, int i, float f, String s) {
        if (applying > 0) {
            return null;
        }
        return sink.dispatch(node, event, i, f, s);
    }
}
