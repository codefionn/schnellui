package io.github.codefionn.schnellui;

/** Maps a Progress range onto the integer range of a ProgressBar. Same as a continuous slider. */
public final class ProgressMath {
    private ProgressMath() {}

    /** The ProgressBar max. 1000 for a real range, 1 when MAX is not above MIN. */
    public static int max(float min, float max) {
        return SliderMath.positions(min, max, 0f);
    }

    /** The ProgressBar progress for VALUE. Out of range values are clamped. */
    public static int progress(float value, float min, float max) {
        return SliderMath.progressFor(value, min, max, 0f);
    }
}
