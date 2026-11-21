package io.github.codefionn.schnellui;

import java.util.HashMap;
import java.util.HashSet;
import java.util.Map;
import java.util.Set;

/**
 * Turns requestUpdate calls from any thread into at most one main-thread task per app.
 *
 * <p>Pure Java, no android.* imports. The poster is injected, so a test runs the tasks by
 * hand. This class never touches a view. It only posts, and the {@link Target} runs on the
 * thread the poster runs tasks on.
 */
public final class UpdateScheduler {
    /** Runs a task on the main thread. */
    public interface Poster {
        void post(Runnable task);
    }

    /** Where an update goes: the activity's applier of one app. */
    public interface Target {
        void update(long app);
    }

    private final Poster poster;
    private final Map<Long, Target> targets = new HashMap<>();
    private final Set<Long> pending = new HashSet<>();

    public UpdateScheduler(Poster poster) {
        this.poster = poster;
    }

    /** Makes the app's updates reach the target. Call on attach. */
    public synchronized void register(long app, Target target) {
        targets.put(app, target);
    }

    /** Drops later updates of the app. Call on detach and on unmount. */
    public synchronized void unregister(long app) {
        targets.remove(app);
    }

    /**
     * Asks for an update of the app. Dropped when no target is registered. A request while
     * a task of the app is waiting joins that task.
     */
    public void request(final long app) {
        synchronized (this) {
            if (!targets.containsKey(app) || !pending.add(app)) {
                return;
            }
        }
        poster.post(() -> run(app));
    }

    private void run(long app) {
        Target t;
        synchronized (this) {
            pending.remove(app);
            t = targets.get(app);
        }
        if (t != null) {
            t.update(app);
        }
    }
}
