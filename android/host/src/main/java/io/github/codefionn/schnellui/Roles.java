package io.github.codefionn.schnellui;

/**
 * Accessibility helpers for the ROLE prop. Tags are the schnellui::a11y::Role values.
 * Pure Java so the tables can be tested on the JVM.
 */
public final class Roles {
    private Roles() {}

    public static final int IMAGE = 6;
    public static final int LIST = 7;
    public static final int PROGRESS_INDICATOR = 9;
    public static final int SWITCH = 10;
    public static final int RADIO = 11;
    public static final int HEADING = 32;

    /** True for roles that announce as a heading. */
    public static boolean isHeading(int role) {
        return role == HEADING;
    }

    /** The accessibility class name for a role, or null to keep the view's own. */
    public static String className(int role) {
        switch (role) {
            case IMAGE:
                return "android.widget.ImageView";
            case LIST:
                return "android.widget.ListView";
            case PROGRESS_INDICATOR:
                return "android.widget.ProgressBar";
            case SWITCH:
                return "android.widget.Switch";
            case RADIO:
                return "android.widget.RadioButton";
            default:
                return null;
        }
    }
}
