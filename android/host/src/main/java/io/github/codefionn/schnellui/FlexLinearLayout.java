package io.github.codefionn.schnellui;

import android.content.Context;
import android.view.View;
import android.widget.LinearLayout;

/**
 * A LinearLayout that can also spread its children for the JUSTIFY values space-between,
 * space-around and space-evenly. Gap is drawn as an invisible divider, so it takes part in
 * the normal layout pass.
 */
final class FlexLinearLayout extends LinearLayout {
    static final int SPACE_BETWEEN = 3;
    static final int SPACE_AROUND = 4;
    static final int SPACE_EVENLY = 5;

    private int spaceMode;

    FlexLinearLayout(Context context, int orientation) {
        super(context);
        setOrientation(orientation);
    }

    /** Sets the spread mode (3 to 5), or 0 for none. */
    void setSpaceMode(int mode) {
        if (mode != spaceMode) {
            spaceMode = mode;
            requestLayout();
        }
    }

    @Override
    protected void onLayout(boolean changed, int l, int t, int r, int b) {
        super.onLayout(changed, l, t, r, b);
        if (spaceMode >= SPACE_BETWEEN) {
            spread();
        }
    }

    private void spread() {
        boolean horizontal = getOrientation() == HORIZONTAL;
        int n = 0;
        View first = null;
        View last = null;
        for (int i = 0; i < getChildCount(); i++) {
            View c = getChildAt(i);
            if (c.getVisibility() == View.GONE) {
                continue;
            }
            if (first == null) {
                first = c;
            }
            last = c;
            n++;
        }
        if (n == 0 || (n == 1 && spaceMode == SPACE_BETWEEN)) {
            return;
        }
        int used;
        int avail;
        if (horizontal) {
            used = last.getRight() - first.getLeft();
            avail = getWidth() - getPaddingLeft() - getPaddingRight();
        } else {
            used = last.getBottom() - first.getTop();
            avail = getHeight() - getPaddingTop() - getPaddingBottom();
        }
        float free = avail - used;
        if (free <= 0) {
            return;
        }
        float between;
        float lead;
        if (spaceMode == SPACE_BETWEEN) {
            between = free / (n - 1);
            lead = 0;
        } else if (spaceMode == SPACE_AROUND) {
            between = free / n;
            lead = between / 2;
        } else {
            between = free / (n + 1);
            lead = between;
        }
        boolean rtl = horizontal && getLayoutDirection() == LAYOUT_DIRECTION_RTL;
        int k = 0;
        for (int i = 0; i < getChildCount(); i++) {
            View c = getChildAt(i);
            if (c.getVisibility() == View.GONE) {
                continue;
            }
            int shift = Math.round(lead + between * k);
            k++;
            if (horizontal) {
                c.offsetLeftAndRight(rtl ? -shift : shift);
            } else {
                c.offsetTopAndBottom(shift);
            }
        }
    }
}
