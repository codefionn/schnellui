package io.github.codefionn.schnellui;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CountDownLatch;
import org.junit.Test;

public class UpdateSchedulerTest {
    /** Queues tasks like a main looper that the test drives by hand. */
    private static final class Queue implements UpdateScheduler.Poster {
        final List<Runnable> tasks = new ArrayList<>();

        @Override
        public synchronized void post(Runnable task) {
            tasks.add(task);
        }

        synchronized int size() {
            return tasks.size();
        }

        void runAll() {
            List<Runnable> copy;
            synchronized (this) {
                copy = new ArrayList<>(tasks);
                tasks.clear();
            }
            for (Runnable r : copy) {
                r.run();
            }
        }
    }

    private static final class Count implements UpdateScheduler.Target {
        final List<Long> calls = new ArrayList<>();
        Thread ranOn;

        @Override
        public void update(long app) {
            calls.add(app);
            ranOn = Thread.currentThread();
        }
    }

    @Test
    public void manyRequestsMakeOneUpdate() {
        Queue q = new Queue();
        Count c = new Count();
        UpdateScheduler s = new UpdateScheduler(q);
        s.register(7, c);
        for (int i = 0; i < 50; i++) {
            s.request(7);
        }
        assertEquals(1, q.size());
        assertTrue(c.calls.isEmpty());
        q.runAll();
        assertEquals(1, c.calls.size());
        assertEquals(7L, (long) c.calls.get(0));
    }

    @Test
    public void requestAfterTheTaskRanMakesAnotherUpdate() {
        Queue q = new Queue();
        Count c = new Count();
        UpdateScheduler s = new UpdateScheduler(q);
        s.register(7, c);
        s.request(7);
        q.runAll();
        s.request(7);
        s.request(7);
        q.runAll();
        assertEquals(2, c.calls.size());
    }

    @Test
    public void appsAreIndependent() {
        Queue q = new Queue();
        Count a = new Count();
        Count b = new Count();
        UpdateScheduler s = new UpdateScheduler(q);
        s.register(1, a);
        s.register(2, b);
        s.request(1);
        s.request(2);
        s.request(1);
        assertEquals(2, q.size());
        q.runAll();
        assertEquals(1, a.calls.size());
        assertEquals(1, b.calls.size());
    }

    @Test
    public void requestWithoutTargetIsDropped() {
        Queue q = new Queue();
        UpdateScheduler s = new UpdateScheduler(q);
        s.request(9);
        assertEquals(0, q.size());
    }

    @Test
    public void requestAfterUnregisterIsDropped() {
        Queue q = new Queue();
        Count c = new Count();
        UpdateScheduler s = new UpdateScheduler(q);
        s.register(7, c);
        s.unregister(7);
        s.request(7);
        assertEquals(0, q.size());
        q.runAll();
        assertTrue(c.calls.isEmpty());
    }

    @Test
    public void unregisterBeforeTheTaskRunsDropsTheUpdate() {
        Queue q = new Queue();
        Count c = new Count();
        UpdateScheduler s = new UpdateScheduler(q);
        s.register(7, c);
        s.request(7);
        s.unregister(7);
        q.runAll();
        assertTrue(c.calls.isEmpty());
        // A later attach works again.
        s.register(7, c);
        s.request(7);
        q.runAll();
        assertEquals(1, c.calls.size());
    }

    @Test
    public void reattachBeforeTheTaskRunsUsesTheNewTarget() {
        Queue q = new Queue();
        Count oldTarget = new Count();
        Count newTarget = new Count();
        UpdateScheduler s = new UpdateScheduler(q);
        s.register(7, oldTarget);
        s.request(7);
        s.unregister(7);
        s.register(7, newTarget);
        q.runAll();
        assertTrue(oldTarget.calls.isEmpty());
        assertEquals(1, newTarget.calls.size());
    }

    @Test
    public void requestsFromOtherThreadsOnlyPostAndNeverRunTheTarget() throws Exception {
        final Queue q = new Queue();
        final Count c = new Count();
        final UpdateScheduler s = new UpdateScheduler(q);
        s.register(7, c);
        final CountDownLatch done = new CountDownLatch(8);
        for (int t = 0; t < 8; t++) {
            new Thread(() -> {
                for (int i = 0; i < 100; i++) {
                    s.request(7);
                }
                done.countDown();
            }).start();
        }
        done.await();
        assertTrue(c.calls.isEmpty());
        assertEquals(1, q.size());
        Thread main = Thread.currentThread();
        q.runAll();
        assertEquals(1, c.calls.size());
        assertEquals(main, c.ranOn);
    }
}
