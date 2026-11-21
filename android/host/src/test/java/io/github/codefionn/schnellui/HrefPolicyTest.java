package io.github.codefionn.schnellui;

import static org.junit.Assert.*;

import org.junit.Test;

public class HrefPolicyTest {
    @Test
    public void allowsWebAndMail() {
        assertTrue(HrefPolicy.allowed("http://example.com"));
        assertTrue(HrefPolicy.allowed("https://example.com/a?b=c"));
        assertTrue(HrefPolicy.allowed("mailto:me@example.com"));
        assertTrue(HrefPolicy.allowed("HTTPS://EXAMPLE.COM"));
    }

    @Test
    public void rejectsEverythingElse() {
        assertFalse(HrefPolicy.allowed(null));
        assertFalse(HrefPolicy.allowed(""));
        assertFalse(HrefPolicy.allowed("javascript:alert(1)"));
        assertFalse(HrefPolicy.allowed("intent://x#Intent;end"));
        assertFalse(HrefPolicy.allowed("file:///etc/passwd"));
        assertFalse(HrefPolicy.allowed("tel:123"));
        assertFalse(HrefPolicy.allowed("example.com"));
        assertFalse(HrefPolicy.allowed(" https://example.com"));
        assertFalse(HrefPolicy.allowed("https:"));
        assertFalse(HrefPolicy.allowed("httpx://a"));
    }
}
