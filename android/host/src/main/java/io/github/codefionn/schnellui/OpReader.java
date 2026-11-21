package io.github.codefionn.schnellui;

import java.nio.charset.StandardCharsets;

/**
 * Decoder of the op buffer described in docs/android-renderer.md.
 *
 * <p>Pure Java, no android.* imports, so it runs in JVM unit tests. The reader walks
 * the buffer and calls the {@link Handler} once per op. On a malformed buffer it throws
 * {@link DecodeException}. Ops before the fault have already been delivered.
 */
public final class OpReader {
    /** Receives decoded ops in buffer order. */
    public interface Handler {
        void create(int id, int kind);

        void createThemed(int id, int kind, int theme);

        void setStr(int id, int prop, String value);

        void setF32(int id, int prop, float value);

        void setI32(int id, int prop, int value);

        void clear(int id, int prop);

        void insert(int parent, int child, int index);

        void remove(int id);

        void setRoot(int id);
    }

    private final byte[] buf;
    private int pos;

    private OpReader(byte[] buf) {
        this.buf = buf;
    }

    /** Decodes the whole buffer into the handler. */
    public static void read(byte[] buf, Handler h) throws DecodeException {
        new OpReader(buf).run(h);
    }

    private void run(Handler h) throws DecodeException {
        if (buf.length < 2) {
            throw new DecodeException("buffer shorter than the 2 byte header");
        }
        int magic = u8();
        if (magic != Codes.MAGIC) {
            throw new DecodeException("bad magic 0x" + Integer.toHexString(magic));
        }
        int version = u8();
        if (version != Codes.VERSION) {
            throw new DecodeException("unsupported version " + version);
        }
        while (pos < buf.length) {
            int at = pos;
            int op = u8();
            switch (op) {
                case Codes.OP_CREATE: {
                    int id = u32();
                    h.create(id, u8());
                    break;
                }
                case Codes.OP_CREATE_THEMED: {
                    int id = u32();
                    int kind = u8();
                    h.createThemed(id, kind, u8());
                    break;
                }
                case Codes.OP_SET_STR: {
                    int id = u32();
                    int prop = u8();
                    h.setStr(id, prop, str());
                    break;
                }
                case Codes.OP_SET_F32: {
                    int id = u32();
                    int prop = u8();
                    h.setF32(id, prop, Float.intBitsToFloat(u32()));
                    break;
                }
                case Codes.OP_SET_I32: {
                    int id = u32();
                    int prop = u8();
                    h.setI32(id, prop, u32());
                    break;
                }
                case Codes.OP_CLEAR: {
                    int id = u32();
                    h.clear(id, u8());
                    break;
                }
                case Codes.OP_INSERT: {
                    int parent = u32();
                    int child = u32();
                    h.insert(parent, child, u32());
                    break;
                }
                case Codes.OP_REMOVE:
                    h.remove(u32());
                    break;
                case Codes.OP_SET_ROOT:
                    h.setRoot(u32());
                    break;
                default:
                    throw new DecodeException(
                            "unknown op 0x" + Integer.toHexString(op) + " at offset " + at);
            }
        }
    }

    private void need(int n) throws DecodeException {
        if (n < 0 || buf.length - pos < n) {
            throw new DecodeException("truncated buffer at offset " + pos);
        }
    }

    private int u8() throws DecodeException {
        need(1);
        return buf[pos++] & 0xff;
    }

    /** Reads 4 bytes little-endian. Ids, indices and i32 values share this. */
    private int u32() throws DecodeException {
        need(4);
        int v = (buf[pos] & 0xff)
                | (buf[pos + 1] & 0xff) << 8
                | (buf[pos + 2] & 0xff) << 16
                | (buf[pos + 3] & 0xff) << 24;
        pos += 4;
        return v;
    }

    private String str() throws DecodeException {
        int len = u32();
        // A length with the top bit set is far beyond any buffer, so need() rejects it.
        need(len);
        String s = new String(buf, pos, len, StandardCharsets.UTF_8);
        pos += len;
        return s;
    }
}
