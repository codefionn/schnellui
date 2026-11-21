package io.github.codefionn.schnellui;

/**
 * Wire codes of the op buffer. These match the tables in docs/android-renderer.md.
 * Do not add values here that the document does not list.
 */
public final class Codes {
    private Codes() {}

    public static final int MAGIC = 0x53;
    public static final int VERSION = 0x01;

    // Ops.
    public static final int OP_CREATE = 0x01;
    public static final int OP_SET_STR = 0x02;
    public static final int OP_SET_F32 = 0x03;
    public static final int OP_SET_I32 = 0x04;
    public static final int OP_CLEAR = 0x05;
    public static final int OP_INSERT = 0x06;
    public static final int OP_REMOVE = 0x07;
    public static final int OP_SET_ROOT = 0x08;

    // View kinds.
    public static final int KIND_ROW = 1;
    public static final int KIND_COLUMN = 2;
    public static final int KIND_STACK = 3;
    public static final int KIND_SCROLL = 4;
    public static final int KIND_TEXT = 5;
    public static final int KIND_BUTTON = 6;
    public static final int KIND_CHECKBOX = 7;
    public static final int KIND_SLIDER = 8;
    public static final int KIND_TEXT_INPUT = 9;
    public static final int KIND_SPACER = 10;

    // Props.
    public static final int PROP_WIDTH = 1;
    public static final int PROP_HEIGHT = 2;
    public static final int PROP_MIN_WIDTH = 3;
    public static final int PROP_MIN_HEIGHT = 4;
    public static final int PROP_MAX_WIDTH = 5;
    public static final int PROP_MAX_HEIGHT = 6;
    public static final int PROP_FILL = 7;
    public static final int PROP_GROW = 8;
    public static final int PROP_BASIS = 9;
    public static final int PROP_PAD_LEFT = 10;
    public static final int PROP_PAD_TOP = 11;
    public static final int PROP_PAD_RIGHT = 12;
    public static final int PROP_PAD_BOTTOM = 13;
    public static final int PROP_VISIBLE = 14;
    public static final int PROP_ENABLED = 15;
    public static final int PROP_JUSTIFY = 20;
    public static final int PROP_ALIGN = 21;
    public static final int PROP_GAP = 22;
    public static final int PROP_WRAP = 23;
    public static final int PROP_SCROLLBAR = 24;
    public static final int PROP_TEXT = 30;
    public static final int PROP_TEXT_SIZE = 31;
    public static final int PROP_TEXT_WRAP = 32;
    public static final int PROP_TEXT_ALIGN = 33;
    public static final int PROP_ELLIPSIS = 34;
    public static final int PROP_ROLE = 35;
    public static final int PROP_A11Y_LABEL = 36;
    public static final int PROP_CHECKED = 40;
    public static final int PROP_VALUE = 41;
    public static final int PROP_MIN = 42;
    public static final int PROP_MAX = 43;
    public static final int PROP_STEP = 44;
    public static final int PROP_PASSWORD = 45;
    public static final int PROP_APPEARANCE = 46;

    // Events.
    public static final int EVENT_CLICK = 1;
    public static final int EVENT_TOGGLE = 2;
    public static final int EVENT_CHANGE = 3;
    public static final int EVENT_INPUT = 4;
}
