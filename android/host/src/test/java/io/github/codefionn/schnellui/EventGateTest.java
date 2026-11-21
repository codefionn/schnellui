package io.github.codefionn.schnellui;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public class EventGateTest {
    private int calls;

    private EventGate gate() {
        return new EventGate((node, event, i, f, s) -> {
            calls++;
            return new byte[] {(byte) node, (byte) event};
        });
    }

    @Test
    public void passesEventsThrough() {
        EventGate g = gate();
        assertArrayEquals(new byte[] {5, 1}, g.fire(5, 1, 0, 0f, null));
        assertEquals(1, calls);
    }

    @Test
    public void suppressesWhileApplying() {
        EventGate g = gate();
        g.begin();
        assertTrue(g.isApplying());
        assertNull(g.fire(5, 1, 0, 0f, null));
        g.end();
        assertFalse(g.isApplying());
        assertEquals(0, calls);
    }

    @Test
    public void nestedApplyingStaysSuppressedUntilTheOutermostEnds() {
        EventGate g = gate();
        g.begin();
        g.begin();
        g.end();
        assertNull(g.fire(1, 1, 0, 0f, null));
        g.end();
        assertEquals(0, calls);
        g.fire(1, 1, 0, 0f, null);
        assertEquals(1, calls);
    }
}
