package io.github.codefionn.schnellui;

/** Picks a readable text color for a background. No android imports. */
public final class Contrast {
    public static final int BLACK = 0xFF000000;
    public static final int WHITE = 0xFFFFFFFF;

    private Contrast() {}

    /** Black text on a light ARGB color, white text on a dark one. */
    public static int textOn(int argb) {
        int r = (argb >> 16) & 0xFF;
        int g = (argb >> 8) & 0xFF;
        int b = argb & 0xFF;
        double luma = (0.299 * r + 0.587 * g + 0.114 * b) / 255.0;
        return luma > 0.6 ? BLACK : WHITE;
    }
}
