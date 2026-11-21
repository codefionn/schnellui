package io.github.codefionn.schnellui;

import static org.junit.Assert.*;

import org.junit.Test;

public class ProgressMathTest {
    @Test
    public void mapsRangeOntoThousandPositions() {
        assertEquals(1000, ProgressMath.max(0, 100));
        assertEquals(0, ProgressMath.progress(0, 0, 100));
        assertEquals(500, ProgressMath.progress(50, 0, 100));
        assertEquals(1000, ProgressMath.progress(100, 0, 100));
    }

    @Test
    public void clampsAndHandlesOffsetRanges() {
        assertEquals(0, ProgressMath.progress(-5, 0, 100));
        assertEquals(1000, ProgressMath.progress(500, 0, 100));
        assertEquals(500, ProgressMath.progress(0, -10, 10));
    }

    @Test
    public void emptyRangeShowsNothing() {
        assertEquals(1, ProgressMath.max(5, 5));
        assertEquals(0, ProgressMath.progress(5, 5, 5));
    }
}
