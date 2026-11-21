package io.github.codefionn.schnellui;

/**
 * The bounds check of a size query, see "Size queries" in docs/android-renderer.md.
 *
 * <p>Pure Java, no android.* imports. An unset bound is NaN.
 */
public final class SizeQuery {
    private SizeQuery() {}

    /** Slack for float error when px is divided by the density. */
    private static final float EPS = 0.001f;

    /**
     * True when the content box (dp) is inside every set bound. Bounds are inclusive. A
     * target that has not been laid out yet always matches.
     */
    public static boolean matches(boolean laidOut, float widthDp, float heightDp,
            float minW, float maxW, float minH, float maxH) {
        if (!laidOut) {
            return true;
        }
        if (!Float.isNaN(minW) && widthDp < minW - EPS) {
            return false;
        }
        if (!Float.isNaN(maxW) && widthDp > maxW + EPS) {
            return false;
        }
        if (!Float.isNaN(minH) && heightDp < minH - EPS) {
            return false;
        }
        return Float.isNaN(maxH) || heightDp <= maxH + EPS;
    }
}
