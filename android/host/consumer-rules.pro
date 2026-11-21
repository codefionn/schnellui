# Rust calls SchnellBridge by name through JNI, so R8 must keep its natives and requestUpdate.
-keep class io.github.codefionn.schnellui.SchnellBridge {
    static native <methods>;
    static void requestUpdate(long);
}
