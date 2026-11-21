package io.github.codefionn.schnellui;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public class SizeQueryTest {
    private static final float N = Float.NaN;

    @Test
    public void noBoundsMatches() {
        assertTrue(SizeQuery.matches(true, 0, 0, N, N, N, N));
        assertTrue(SizeQuery.matches(true, 5000, 5000, N, N, N, N));
    }

    @Test
    public void minWidthIsInclusive() {
        assertTrue(SizeQuery.matches(true, 600, 100, 600, N, N, N));
        assertFalse(SizeQuery.matches(true, 599.5f, 100, 600, N, N, N));
    }

    @Test
    public void maxWidthIsInclusive() {
        assertTrue(SizeQuery.matches(true, 600, 100, N, 600, N, N));
        assertFalse(SizeQuery.matches(true, 600.5f, 100, N, 600, N, N));
    }

    @Test
    public void minHeightIsInclusive() {
        assertTrue(SizeQuery.matches(true, 100, 400, N, N, 400, N));
        assertFalse(SizeQuery.matches(true, 100, 399.5f, N, N, 400, N));
    }

    @Test
    public void maxHeightIsInclusive() {
        assertTrue(SizeQuery.matches(true, 100, 400, N, N, N, 400));
        assertFalse(SizeQuery.matches(true, 100, 400.5f, N, N, N, 400));
    }

    @Test
    public void everySetBoundMustHold() {
        assertTrue(SizeQuery.matches(true, 500, 300, 400, 600, 200, 400));
        assertFalse(SizeQuery.matches(true, 500, 500, 400, 600, 200, 400));
        assertFalse(SizeQuery.matches(true, 700, 300, 400, 600, 200, 400));
    }

    @Test
    public void floatErrorAtTheEdgeStillMatches() {
        assertTrue(SizeQuery.matches(true, 640.00006f, 100, N, 640, N, N));
    }

    @Test
    public void unlaidOutTargetMatches() {
        assertTrue(SizeQuery.matches(false, 0, 0, 600, N, 400, N));
        assertTrue(SizeQuery.matches(false, 10, 10, N, 5, N, 5));
    }
}
