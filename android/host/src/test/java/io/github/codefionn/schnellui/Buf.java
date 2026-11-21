package io.github.codefionn.schnellui;

import java.io.ByteArrayOutputStream;
import java.nio.charset.StandardCharsets;

/** Hand-written encoder for tests. It follows the op buffer table in the doc. */
final class Buf {
    private final ByteArrayOutputStream out = new ByteArrayOutputStream();

    static Buf withHeader() {
        return new Buf().u8(0x53).u8(0x01);
    }

    Buf u8(int v) {
        out.write(v);
        return this;
    }

    Buf u32(int v) {
        out.write(v);
        out.write(v >>> 8);
        out.write(v >>> 16);
        out.write(v >>> 24);
        return this;
    }

    Buf f32(float v) {
        return u32(Float.floatToRawIntBits(v));
    }

    Buf str(String s) {
        byte[] b = s.getBytes(StandardCharsets.UTF_8);
        u32(b.length);
        out.write(b, 0, b.length);
        return this;
    }

    byte[] bytes() {
        return out.toByteArray();
    }
}
