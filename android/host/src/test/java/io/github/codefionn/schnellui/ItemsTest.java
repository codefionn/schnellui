package io.github.codefionn.schnellui;

import static org.junit.Assert.*;

import org.junit.Test;

public class ItemsTest {
    @Test
    public void emptyStringIsNoOptions() {
        assertEquals(0, Items.split("").length);
        assertEquals(0, Items.split(null).length);
    }

    @Test
    public void splitsOnUnitSeparator() {
        String sep = String.valueOf((char) 0x1f);
        assertArrayEquals(new String[] {"one"}, Items.split("one"));
        assertArrayEquals(new String[] {"a", "b c", "d"}, Items.split("a" + sep + "b c" + sep + "d"));
    }

    @Test
    public void keepsEmptyOptions() {
        String sep = String.valueOf((char) 0x1f);
        assertArrayEquals(new String[] {"", ""}, Items.split(sep));
        assertArrayEquals(new String[] {"a", ""}, Items.split("a" + sep));
        assertArrayEquals(new String[] {"", "a"}, Items.split(sep + "a"));
    }

    @Test
    public void effectiveIndexShowsFirstOptionForNoSelection() {
        assertEquals(0, Items.effectiveIndex(-1, 3));
        assertEquals(2, Items.effectiveIndex(2, 3));
        assertEquals(0, Items.effectiveIndex(3, 3));
        assertEquals(-1, Items.effectiveIndex(0, 0));
    }

    @Test
    public void selectionMatchingTheAppliedIndexIsNotAUserPick() {
        assertFalse(Items.isUserPick(2, 2));
        assertTrue(Items.isUserPick(1, 2));
        assertFalse(Items.isUserPick(-1, 2));
    }
}
