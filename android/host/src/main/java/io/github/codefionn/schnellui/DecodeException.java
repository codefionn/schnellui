package io.github.codefionn.schnellui;

/** The op buffer is malformed: bad header, truncated field or unknown op code. */
public final class DecodeException extends Exception {
    public DecodeException(String message) {
        super(message);
    }
}
