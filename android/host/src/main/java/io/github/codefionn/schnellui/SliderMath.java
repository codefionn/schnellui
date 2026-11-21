package io.github.codefionn.schnellui;

/** Maps a slider value range onto the integer positions of a SeekBar and back. */
public final class SliderMath {
    /** Number of positions for a continuous slider (STEP 0). */
    public static final int CONTINUOUS_POSITIONS = 1000;

    private SliderMath() {}

    /** The SeekBar max: the number of steps, or 1000 when continuous. At least 1. */
    public static int positions(float min, float max, float step) {
        if (!(max > min)) {
            return 1;
        }
        if (step > 0) {
            return Math.max(1, Math.round((max - min) / step));
        }
        return CONTINUOUS_POSITIONS;
    }

    public static int progressFor(float value, float min, float max, float step) {
        int n = positions(min, max, step);
        if (!(max > min)) {
            return 0;
        }
        float t = step > 0 ? (value - min) / step : (value - min) / (max - min) * n;
        return Math.max(0, Math.min(n, Math.round(t)));
    }

    public static float valueFor(int progress, float min, float max, float step) {
        int n = positions(min, max, step);
        if (!(max > min)) {
            return min;
        }
        float v = step > 0 ? min + progress * step : min + (float) progress / n * (max - min);
        return Math.max(min, Math.min(max, v));
    }
}
