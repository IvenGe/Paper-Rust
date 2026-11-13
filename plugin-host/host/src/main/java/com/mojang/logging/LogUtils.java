package com.mojang.logging;

import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * Stand-in for the part of Mojang's logging library that Paper's plugin loader uses.
 *
 * <p>Paper's copy adds {@code getClassLogger()}; both name the logger after the calling class.
 */
public final class LogUtils {
    private static final StackWalker STACK_WALKER = StackWalker.getInstance(StackWalker.Option.RETAIN_CLASS_REFERENCE);

    private LogUtils() {
    }

    public static Logger getLogger() {
        return LoggerFactory.getLogger(STACK_WALKER.getCallerClass());
    }

    public static Logger getClassLogger() {
        return LoggerFactory.getLogger(STACK_WALKER.getCallerClass().getSimpleName());
    }
}
