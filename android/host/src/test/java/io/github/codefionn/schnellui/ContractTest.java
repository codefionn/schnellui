package io.github.codefionn.schnellui;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;
import static org.junit.Assume.assumeTrue;

import java.io.File;
import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.List;
import org.junit.Test;

/** Checks the host against docs/android-renderer.md. */
public class ContractTest {
    private static String doc() throws Exception {
        for (File dir = new File("").getAbsoluteFile(); dir != null; dir = dir.getParentFile()) {
            File f = new File(dir, "docs/android-renderer.md");
            if (f.isFile()) {
                return new String(Files.readAllBytes(f.toPath()), StandardCharsets.UTF_8);
            }
        }
        assumeTrue("docs/android-renderer.md not found", false);
        return null;
    }

    /** Rows of the table under the heading, as cells. Only rows whose first cell is a code. */
    private static List<String[]> table(String doc, String heading) {
        int at = doc.indexOf("## " + heading);
        assertTrue(heading, at >= 0);
        List<String[]> rows = new ArrayList<>();
        boolean started = false;
        for (String line : doc.substring(at).split("\n")) {
            if (line.startsWith("|")) {
                started = true;
                String[] cells = line.split("\\|");
                for (int i = 0; i < cells.length; i++) {
                    cells[i] = cells[i].trim();
                }
                if (cells.length > 2 && cells[1].matches("(0x)?[0-9a-fA-F]+")) {
                    rows.add(cells);
                }
            } else if (started) {
                break;
            }
        }
        return rows;
    }

    private static int code(String cell) {
        return cell.startsWith("0x")
                ? Integer.parseInt(cell.substring(2), 16)
                : Integer.parseInt(cell);
    }

    private static int constant(String name) throws Exception {
        Field f = Codes.class.getField(name);
        assertTrue(name, Modifier.isStatic(f.getModifiers()));
        return f.getInt(null);
    }

    private static String ident(String cell) {
        return cell.replace("`", "").trim().replaceAll("([a-z])([A-Z])", "$1_$2").toUpperCase().replace('-', '_').replace(' ', '_');
    }

    @Test
    public void opCodesMatchDoc() throws Exception {
        List<String[]> rows = table(doc(), "Op buffer");
        assertEquals(9, rows.size());
        for (String[] r : rows) {
            assertEquals(r[2], code(r[1]), constant("OP_" + ident(r[2])));
        }
    }

    @Test
    public void kindCodesMatchDoc() throws Exception {
        List<String[]> rows = table(doc(), "View kinds");
        assertEquals(27, rows.size());
        for (String[] r : rows) {
            assertEquals(r[2], code(r[1]), constant("KIND_" + ident(r[2])));
        }
    }

    @Test
    public void propCodesMatchDoc() throws Exception {
        List<String[]> rows = table(doc(), "Properties");
        assertEquals(44, rows.size());
        for (String[] r : rows) {
            assertEquals(r[2], code(r[1]), constant("PROP_" + ident(r[2])));
        }
    }

    @Test
    public void eventCodesMatchDoc() throws Exception {
        List<String[]> rows = table(doc(), "Events");
        assertEquals(6, rows.size());
        for (String[] r : rows) {
            assertEquals(r[2], code(r[1]), constant("EVENT_" + ident(r[2])));
        }
    }

    @Test
    public void noConstantBeyondTheDoc() {
        int count = 0;
        for (Field f : Codes.class.getFields()) {
            String n = f.getName();
            if (n.startsWith("OP_") || n.startsWith("KIND_") || n.startsWith("PROP_")
                    || n.startsWith("EVENT_")) {
                count++;
            }
        }
        assertEquals(9 + 27 + 44 + 6, count);
        assertEquals(0x53, Codes.MAGIC);
        assertEquals(0x01, Codes.VERSION);
    }

    @Test
    public void bridgeSignaturesMatchDoc() throws Exception {
        assertEquals("io.github.codefionn.schnellui.SchnellBridge",
                SchnellBridge.class.getName());
        check("nativeMount", long.class, float.class, float.class, float.class, boolean.class);
        check("nativeRender", byte[].class, long.class);
        check("nativeDispatch", byte[].class,
                long.class, int.class, int.class, int.class, float.class, String.class);
        check("nativeResize", byte[].class, long.class, float.class, float.class);
        check("nativeUnmount", void.class, long.class);
        check("nativeUpdate", byte[].class, long.class);
        Method req = SchnellBridge.class.getDeclaredMethod("requestUpdate", long.class);
        assertTrue(Modifier.isStatic(req.getModifiers()));
        assertFalse(Modifier.isNative(req.getModifiers()));
        assertEquals(void.class, req.getReturnType());
        int natives = 0;
        for (Method m : SchnellBridge.class.getDeclaredMethods()) {
            if (Modifier.isNative(m.getModifiers())) {
                natives++;
            }
        }
        assertEquals(6, natives);
    }

    @Test
    public void consumerRulesKeepTheBridge() throws Exception {
        File rules = new File("consumer-rules.pro");
        assumeTrue(rules.exists());
        String text = new String(Files.readAllBytes(rules.toPath()), StandardCharsets.UTF_8);
        assertTrue(text.contains("-keep class io.github.codefionn.schnellui.SchnellBridge"));
        assertTrue(text.contains("static void requestUpdate(long);"));
    }

    private static void check(String name, Class<?> ret, Class<?>... params) throws Exception {
        Method m = SchnellBridge.class.getDeclaredMethod(name, params);
        assertTrue(name, Modifier.isNative(m.getModifiers()));
        assertTrue(name, Modifier.isStatic(m.getModifiers()));
        assertEquals(name, ret, m.getReturnType());
    }
}
