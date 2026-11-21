package io.github.codefionn.schnellui;

import java.util.Base64;
import java.util.Locale;

/** Pure helpers for the Image SRC prop. No android imports. */
public final class ImageSrc {
    public static final int NONE = 0;
    public static final int DATA = 1;
    public static final int FILE = 2;
    public static final int ASSET = 3;
    /** Any other scheme. The host logs it and shows nothing. */
    public static final int OTHER = 4;

    private static final String ASSET_PREFIX = "asset:///";

    private ImageSrc() {}

    public static int classify(String src) {
        if (src == null || src.isEmpty()) {
            return NONE;
        }
        String lower = src.toLowerCase(Locale.ROOT);
        if (lower.startsWith("data:")) {
            return DATA;
        }
        if (lower.startsWith(ASSET_PREFIX)) {
            return ASSET;
        }
        if (src.charAt(0) == '/') {
            return FILE;
        }
        return OTHER;
    }

    /** The asset name of an asset:/// URL, or null when it is not one or the name is empty. */
    public static String assetName(String src) {
        if (classify(src) != ASSET) {
            return null;
        }
        String name = src.substring(ASSET_PREFIX.length());
        return name.isEmpty() ? null : name;
    }

    /**
     * The bytes of a data URL with base64 PNG, JPEG or WebP content. Null for anything else,
     * including bad base64.
     */
    public static byte[] decodeData(String src) {
        if (classify(src) != DATA) {
            return null;
        }
        int comma = src.indexOf(',');
        if (comma < 0) {
            return null;
        }
        String[] head = src.substring("data:".length(), comma).toLowerCase(Locale.ROOT).split(";");
        boolean base64 = false;
        for (int i = 1; i < head.length; i++) {
            if (head[i].trim().equals("base64")) {
                base64 = true;
            }
        }
        String mime = head.length == 0 ? "" : head[0].trim();
        if (!base64 || !(mime.equals("image/png") || mime.equals("image/jpeg")
                || mime.equals("image/jpg") || mime.equals("image/webp"))) {
            return null;
        }
        try {
            byte[] out = Base64.getMimeDecoder().decode(src.substring(comma + 1));
            return out.length == 0 ? null : out;
        } catch (IllegalArgumentException e) {
            return null;
        }
    }
}
