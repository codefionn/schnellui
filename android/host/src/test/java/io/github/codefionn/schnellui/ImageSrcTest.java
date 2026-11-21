package io.github.codefionn.schnellui;

import static org.junit.Assert.*;

import org.junit.Test;

public class ImageSrcTest {
    // 1x1 transparent PNG.
    private static final String PNG = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

    @Test
    public void classifiesSchemes() {
        assertEquals(ImageSrc.NONE, ImageSrc.classify(null));
        assertEquals(ImageSrc.NONE, ImageSrc.classify(""));
        assertEquals(ImageSrc.DATA, ImageSrc.classify("data:image/png;base64,AAAA"));
        assertEquals(ImageSrc.DATA, ImageSrc.classify("DATA:image/png;base64,AAAA"));
        assertEquals(ImageSrc.FILE, ImageSrc.classify("/sdcard/a.png"));
        assertEquals(ImageSrc.ASSET, ImageSrc.classify("asset:///logo.png"));
        assertEquals(ImageSrc.OTHER, ImageSrc.classify("https://example.com/a.png"));
        assertEquals(ImageSrc.OTHER, ImageSrc.classify("file:///a.png"));
        assertEquals(ImageSrc.OTHER, ImageSrc.classify("relative/a.png"));
    }

    @Test
    public void assetNames() {
        assertEquals("logo.png", ImageSrc.assetName("asset:///logo.png"));
        assertEquals("img/a.webp", ImageSrc.assetName("asset:///img/a.webp"));
        assertNull(ImageSrc.assetName("asset:///"));
        assertNull(ImageSrc.assetName("/a.png"));
    }

    @Test
    public void decodesBase64ImageData() {
        byte[] b = ImageSrc.decodeData("data:image/png;base64," + PNG);
        assertNotNull(b);
        assertEquals((byte) 0x89, b[0]);
        assertEquals('P', b[1]);
        assertNotNull(ImageSrc.decodeData("data:image/jpeg;base64," + PNG));
        assertNotNull(ImageSrc.decodeData("data:image/webp;base64," + PNG));
    }

    @Test
    public void rejectsOtherData() {
        assertNull(ImageSrc.decodeData("data:image/png,rawtext"));
        assertNull(ImageSrc.decodeData("data:image/svg+xml;base64," + PNG));
        assertNull(ImageSrc.decodeData("data:image/png;base64,!!!not base64"));
        assertNull(ImageSrc.decodeData("data:image/png;base64,"));
        assertNull(ImageSrc.decodeData("data:image/png;base64"));
        assertNull(ImageSrc.decodeData("/a.png"));
    }
}
