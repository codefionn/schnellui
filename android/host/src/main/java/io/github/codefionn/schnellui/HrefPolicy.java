package io.github.codefionn.schnellui;

import java.util.Locale;

/** Which HREF values a Link may open. No android imports. */
public final class HrefPolicy {
    private HrefPolicy() {}

    /** True for http, https and mailto URLs. The scheme match ignores case. */
    public static boolean allowed(String href) {
        if (href == null) {
            return false;
        }
        int colon = href.indexOf(':');
        if (colon <= 0 || colon == href.length() - 1) {
            return false;
        }
        String scheme = href.substring(0, colon).toLowerCase(Locale.ROOT);
        return scheme.equals("http") || scheme.equals("https") || scheme.equals("mailto");
    }
}
