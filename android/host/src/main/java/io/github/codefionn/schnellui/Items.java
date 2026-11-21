package io.github.codefionn.schnellui;

import java.util.ArrayList;
import java.util.List;

/** Pure helpers for Dropdown: the ITEMS string and the selected index. No android imports. */
public final class Items {
    /** Separates option labels in the ITEMS prop. This is U+001F. */
    public static final char SEPARATOR = (char) 0x1f;

    private Items() {}

    /** Splits ITEMS into labels. The empty string means no options, not one empty option. */
    public static String[] split(String items) {
        if (items == null || items.isEmpty()) {
            return new String[0];
        }
        List<String> out = new ArrayList<>();
        int start = 0;
        for (int i = items.indexOf(SEPARATOR); i >= 0; i = items.indexOf(SEPARATOR, start)) {
            out.add(items.substring(start, i));
            start = i + 1;
        }
        out.add(items.substring(start));
        return out.toArray(new String[0]);
    }

    /**
     * The index a Spinner shows for SELECTED_INDEX. A Spinner cannot show nothing, so -1 and
     * out of range values show the first option. With no options the result is -1.
     */
    public static int effectiveIndex(int selected, int count) {
        if (count <= 0) {
            return -1;
        }
        return selected >= 0 && selected < count ? selected : 0;
    }

    /**
     * True when a Spinner selection callback is a user pick. A Spinner reports selections that
     * the host made itself later, from its layout pass. Those match the index the host applied.
     */
    public static boolean isUserPick(int position, int lastApplied) {
        return position >= 0 && position != lastApplied;
    }
}
