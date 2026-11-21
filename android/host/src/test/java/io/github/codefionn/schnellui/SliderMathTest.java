package io.github.codefionn.schnellui;

import static org.junit.Assert.assertEquals;

import org.junit.Test;

public class SliderMathTest {
    private static final float EPS = 1e-4f;

    @Test
    public void continuousUsesThousandPositions() {
        assertEquals(1000, SliderMath.positions(0, 100, 0));
        assertEquals(500, SliderMath.progressFor(50, 0, 100, 0));
        assertEquals(50f, SliderMath.valueFor(500, 0, 100, 0), EPS);
    }

    @Test
    public void steppedUsesStepCount() {
        assertEquals(10, SliderMath.positions(0, 100, 10));
        assertEquals(3, SliderMath.progressFor(30, 0, 100, 10));
        assertEquals(30f, SliderMath.valueFor(3, 0, 100, 10), EPS);
    }

    @Test
    public void nonZeroMin() {
        assertEquals(4, SliderMath.positions(-2, 2, 1));
        assertEquals(2, SliderMath.progressFor(0, -2, 2, 1));
        assertEquals(-1f, SliderMath.valueFor(1, -2, 2, 1), EPS);
    }

    @Test
    public void clampsOutOfRange() {
        assertEquals(1000, SliderMath.progressFor(500, 0, 100, 0));
        assertEquals(0, SliderMath.progressFor(-5, 0, 100, 0));
        assertEquals(100f, SliderMath.valueFor(1000, 0, 100, 0), EPS);
    }

    @Test
    public void emptyRangeIsSafe() {
        assertEquals(1, SliderMath.positions(5, 5, 0));
        assertEquals(0, SliderMath.progressFor(5, 5, 5, 0));
        assertEquals(5f, SliderMath.valueFor(0, 5, 5, 0), EPS);
    }

    @Test
    public void roundTripOnStepGrid() {
        for (int p = 0; p <= 20; p++) {
            float v = SliderMath.valueFor(p, 0, 10, 0.5f);
            assertEquals(p, SliderMath.progressFor(v, 0, 10, 0.5f));
        }
    }
}
