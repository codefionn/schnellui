package io.github.codefionn.schnellui;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public class RolesTest {
    @Test
    public void columnHeaderIsHeading() {
        assertTrue(Roles.isHeading(21));
        assertFalse(Roles.isHeading(2));
        assertFalse(Roles.isHeading(-1));
    }

    @Test
    public void classNames() {
        assertEquals("android.widget.ImageView", Roles.className(6));
        assertNull(Roles.className(0));
    }
}
