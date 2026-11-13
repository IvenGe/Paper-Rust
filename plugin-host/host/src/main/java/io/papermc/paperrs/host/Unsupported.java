package io.papermc.paperrs.host;

/**
 * The exception thrown by every API method Paper-RS does not implement yet.
 *
 * <p>The message names the method, so a plugin that reaches an unimplemented feature fails
 * with an actionable error instead of silently doing nothing.
 */
public final class Unsupported extends UnsupportedOperationException {
    private Unsupported(String message) {
        super(message);
    }

    /** {@code method} is written as {@code Type#name}. */
    public static Unsupported method(String method) {
        return new Unsupported(method + " is not supported by Paper-RS yet");
    }

    public static Unsupported feature(String description) {
        return new Unsupported(description + " is not supported by Paper-RS yet");
    }
}
