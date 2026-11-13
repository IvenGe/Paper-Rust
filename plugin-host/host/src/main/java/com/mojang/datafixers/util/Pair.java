package com.mojang.datafixers.util;

/** The slice of DataFixerUpper's {@code Pair} that Paper's plugin graph code uses. */
public record Pair<F, S>(F first, S second) {
    public static <F, S> Pair<F, S> of(F first, S second) {
        return new Pair<>(first, second);
    }

    public F getFirst() {
        return this.first;
    }

    public S getSecond() {
        return this.second;
    }
}
