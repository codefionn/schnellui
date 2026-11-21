package io.github.codefionn.schnellui;

import static org.junit.Assert.*;

import org.junit.Test;

public class ContrastTest {
    @Test
    public void picksReadableText() {
        assertEquals(Contrast.WHITE, Contrast.textOn(0xFF1565C0));
        assertEquals(Contrast.BLACK, Contrast.textOn(0xFF80CBC4));
        assertEquals(Contrast.BLACK, Contrast.textOn(0xFFFFFFFF));
        assertEquals(Contrast.WHITE, Contrast.textOn(0xFF000000));
    }
}
