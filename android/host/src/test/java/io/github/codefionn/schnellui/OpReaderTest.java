package io.github.codefionn.schnellui;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;
import static org.junit.Assert.fail;

import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import org.junit.Test;

public class OpReaderTest {
    /** Records every op as a readable string. */
    private static final class Rec implements OpReader.Handler {
        final List<String> ops = new ArrayList<>();

        public void create(int id, int kind) {
            ops.add("create " + id + " " + kind);
        }

        public void setStr(int id, int prop, String v) {
            ops.add("str " + id + " " + prop + " " + v);
        }

        public void setF32(int id, int prop, float v) {
            ops.add("f32 " + id + " " + prop + " " + v);
        }

        public void setI32(int id, int prop, int v) {
            ops.add("i32 " + id + " " + prop + " " + v);
        }

        public void clear(int id, int prop) {
            ops.add("clear " + id + " " + prop);
        }

        public void insert(int parent, int child, int index) {
            ops.add("insert " + parent + " " + child + " " + index);
        }

        public void remove(int id) {
            ops.add("remove " + id);
        }

        public void setRoot(int id) {
            ops.add("root " + id);
        }
    }

    private static List<String> decode(byte[] b) throws DecodeException {
        Rec r = new Rec();
        OpReader.read(b, r);
        return r.ops;
    }

    private static void assertFails(byte[] b, String part) {
        try {
            decode(b);
            fail("expected DecodeException");
        } catch (DecodeException e) {
            assertTrue(e.getMessage(), e.getMessage().contains(part));
        }
    }

    @Test
    public void headerOnlyIsEmpty() throws Exception {
        assertTrue(decode(Buf.withHeader().bytes()).isEmpty());
    }

    @Test
    public void rejectsBadHeader() {
        assertFails(new byte[0], "header");
        assertFails(new byte[] {0x53}, "header");
        assertFails(new Buf().u8(0x54).u8(1).bytes(), "magic");
        assertFails(new Buf().u8(0x53).u8(2).bytes(), "version");
    }

    @Test
    public void createWithEveryKind() throws Exception {
        Buf b = Buf.withHeader();
        for (int kind = 1; kind <= 10; kind++) {
            b.u8(0x01).u32(kind + 100).u8(kind);
        }
        List<String> ops = decode(b.bytes());
        assertEquals(10, ops.size());
        assertEquals("create 101 1", ops.get(0));
        assertEquals("create 110 10", ops.get(9));
    }

    @Test
    public void setStr() throws Exception {
        byte[] b = Buf.withHeader().u8(0x02).u32(7).u8(30).str("hello").bytes();
        assertEquals(Arrays.asList("str 7 30 hello"), decode(b));
    }

    @Test
    public void emptyString() throws Exception {
        byte[] b = Buf.withHeader().u8(0x02).u32(7).u8(30).str("").bytes();
        assertEquals(Arrays.asList("str 7 30 "), decode(b));
    }

    @Test
    public void setF32() throws Exception {
        byte[] b = Buf.withHeader().u8(0x03).u32(2).u8(1).f32(12.5f).bytes();
        assertEquals(Arrays.asList("f32 2 1 12.5"), decode(b));
    }

    @Test
    public void f32IsLittleEndianIeee() throws Exception {
        // 1.0f is 0x3f800000, so the bytes are 00 00 80 3f.
        byte[] b = {0x53, 0x01, 0x03, 1, 0, 0, 0, 8, 0x00, 0x00, (byte) 0x80, 0x3f};
        assertEquals(Arrays.asList("f32 1 8 1.0"), decode(b));
    }

    @Test
    public void setI32IncludingNegative() throws Exception {
        byte[] b = Buf.withHeader()
                .u8(0x04).u32(3).u8(20).u32(5)
                .u8(0x04).u32(3).u8(35).u32(-1)
                .bytes();
        assertEquals(Arrays.asList("i32 3 20 5", "i32 3 35 -1"), decode(b));
    }

    @Test
    public void integersAreLittleEndian() throws Exception {
        byte[] b = {0x53, 0x01, 0x04, 0x78, 0x56, 0x34, 0x12, 20, 1, 0, 0, 0};
        assertEquals(Arrays.asList("i32 " + 0x12345678 + " 20 1"), decode(b));
    }

    @Test
    public void clear() throws Exception {
        byte[] b = Buf.withHeader().u8(0x05).u32(9).u8(14).bytes();
        assertEquals(Arrays.asList("clear 9 14"), decode(b));
    }

    @Test
    public void insert() throws Exception {
        byte[] b = Buf.withHeader().u8(0x06).u32(1).u32(2).u32(3).bytes();
        assertEquals(Arrays.asList("insert 1 2 3"), decode(b));
    }

    @Test
    public void remove() throws Exception {
        byte[] b = Buf.withHeader().u8(0x07).u32(4).bytes();
        assertEquals(Arrays.asList("remove 4"), decode(b));
    }

    @Test
    public void setRoot() throws Exception {
        byte[] b = Buf.withHeader().u8(0x08).u32(1).bytes();
        assertEquals(Arrays.asList("root 1"), decode(b));
    }

    @Test
    public void opsKeepBufferOrder() throws Exception {
        byte[] b = Buf.withHeader()
                .u8(0x01).u32(1).u8(2)
                .u8(0x01).u32(2).u8(5)
                .u8(0x02).u32(2).u8(30).str("x")
                .u8(0x06).u32(1).u32(2).u32(0)
                .u8(0x08).u32(1)
                .bytes();
        assertEquals(Arrays.asList(
                "create 1 2", "create 2 5", "str 2 30 x", "insert 1 2 0", "root 1"),
                decode(b));
    }

    @Test
    public void multiByteUtf8() throws Exception {
        // 2, 3 and 4 byte sequences. The length prefix counts bytes, not chars.
        String s = "héllo 日本語 😀";
        byte[] bytes = s.getBytes(StandardCharsets.UTF_8);
        assertTrue(bytes.length > s.length());
        byte[] b = Buf.withHeader().u8(0x02).u32(1).u8(30).str(s).u8(0x07).u32(1).bytes();
        List<String> ops = decode(b);
        assertEquals("str 1 30 " + s, ops.get(0));
        assertEquals("remove 1", ops.get(1));
    }

    @Test
    public void multiByteBytesWrittenByHand() throws Exception {
        // U+00E9 is C3 A9, so the length is 2 for one char.
        byte[] b = {0x53, 0x01, 0x02, 1, 0, 0, 0, 30, 2, 0, 0, 0, (byte) 0xc3, (byte) 0xa9};
        assertEquals(Arrays.asList("str 1 30 é"), decode(b));
    }

    @Test
    public void truncatedInEveryPosition() throws Exception {
        byte[] full = Buf.withHeader().u8(0x02).u32(7).u8(30).str("héllo").bytes();
        for (int n = 3; n < full.length; n++) {
            assertFails(Arrays.copyOf(full, n), "truncated");
        }
        assertEquals(1, decode(full).size());
    }

    @Test
    public void truncatedFixedOps() {
        assertFails(new Buf().u8(0x53).u8(1).u8(0x01).u32(1).bytes(), "truncated");
        assertFails(new Buf().u8(0x53).u8(1).u8(0x03).u32(1).u8(1).u8(0).bytes(), "truncated");
        assertFails(new Buf().u8(0x53).u8(1).u8(0x06).u32(1).u32(2).bytes(), "truncated");
        assertFails(new Buf().u8(0x53).u8(1).u8(0x07).u8(1).bytes(), "truncated");
    }

    @Test
    public void stringLengthBeyondBuffer() {
        byte[] b = Buf.withHeader().u8(0x02).u32(1).u8(30).u32(100).u8('a').bytes();
        assertFails(b, "truncated");
        // A length with the top bit set must not overflow.
        byte[] huge = Buf.withHeader().u8(0x02).u32(1).u8(30).u32(0x80000000).bytes();
        assertFails(huge, "truncated");
    }

    @Test
    public void unknownOpIsError() {
        assertFails(Buf.withHeader().u8(0x00).bytes(), "unknown op");
        assertFails(Buf.withHeader().u8(0x09).u32(1).bytes(), "unknown op");
        assertFails(Buf.withHeader().u8(0xff).bytes(), "unknown op");
    }

    @Test
    public void opsBeforeAFaultAreDelivered() {
        Rec r = new Rec();
        byte[] b = Buf.withHeader().u8(0x07).u32(4).u8(0x7f).bytes();
        try {
            OpReader.read(b, r);
            fail();
        } catch (DecodeException e) {
            assertEquals(Arrays.asList("remove 4"), r.ops);
        }
    }

    @Test
    public void unknownPropAndKindStillDecode() throws Exception {
        // Skipping unknown props and kinds is the applier's job. The decoder passes them on.
        byte[] b = Buf.withHeader().u8(0x01).u32(1).u8(99).u8(0x04).u32(1).u8(200).u32(0).bytes();
        assertEquals(Arrays.asList("create 1 99", "i32 1 200 0"), decode(b));
    }
}
