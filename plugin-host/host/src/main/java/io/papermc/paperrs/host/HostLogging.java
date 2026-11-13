package io.papermc.paperrs.host;

import java.io.PrintStream;
import java.io.PrintWriter;
import java.io.StringWriter;
import java.nio.charset.StandardCharsets;
import java.util.logging.Handler;
import java.util.logging.Level;
import java.util.logging.LogManager;
import java.util.logging.LogRecord;
import java.util.logging.Logger;

/**
 * Sends java.util.logging (and so SLF4J, plugin loggers and Paper's component loggers) to
 * standard output as one {@code LEVEL|message} line per line of text. The Rust server reads
 * those lines and writes them into its own log with the matching level.
 */
public final class HostLogging {
    private HostLogging() {
    }

    public static void install() {
        // The original stdout, before anything wraps it.
        final PrintStream out = new PrintStream(new java.io.FileOutputStream(java.io.FileDescriptor.out), true, StandardCharsets.UTF_8);
        LogManager.getLogManager().reset();
        final Logger root = Logger.getLogger("");
        root.setLevel(Level.INFO);
        root.addHandler(new Handler() {
            @Override
            public void publish(final LogRecord record) {
                if (!this.isLoggable(record)) {
                    return;
                }
                final String level = record.getLevel().intValue() >= Level.SEVERE.intValue() ? "SEVERE"
                    : record.getLevel().intValue() >= Level.WARNING.intValue() ? "WARNING"
                    : record.getLevel().intValue() >= Level.INFO.intValue() ? "INFO" : "FINE";
                String message = record.getMessage() == null ? "" : record.getMessage();
                if (record.getParameters() != null && record.getParameters().length > 0) {
                    try {
                        message = java.text.MessageFormat.format(message, record.getParameters());
                    } catch (IllegalArgumentException ignored) {
                        // not a format string
                    }
                }
                final String name = record.getLoggerName();
                // Plugin loggers are named after the plugin; Paper shows that name in brackets.
                final boolean prefixed = name != null && !name.isEmpty() && !name.equals("Minecraft") && !name.contains(".") && !message.startsWith("[");
                final StringBuilder text = new StringBuilder(prefixed ? "[" + name + "] " + message : message);
                if (record.getThrown() != null) {
                    final StringWriter trace = new StringWriter();
                    record.getThrown().printStackTrace(new PrintWriter(trace));
                    text.append('\n').append(trace.toString().stripTrailing());
                }
                synchronized (out) {
                    for (String line : text.toString().split("\\R")) {
                        out.println(level + "|" + line);
                    }
                }
            }

            @Override
            public void flush() {
                out.flush();
            }

            @Override
            public void close() {
            }
        });
    }
}
