package io.papermc.paper.block.bed;

import net.kyori.adventure.text.Component;
import org.jspecify.annotations.Nullable;

/**
 * The problems that keep a player out of bed ({@link BedEnterProblem}'s constants), each with
 * the message vanilla shows. The bridge is package-private in the API, hence this package.
 */
public final class HostBedEnterActionBridge implements BedEnterActionBridge {
    private record Problem(@Nullable Component errorMessage) implements BedEnterProblem {
    }

    @Override
    public BedEnterProblem createTooFarAwayProblem() {
        return new Problem(Component.translatable("block.minecraft.bed.too_far_away"));
    }

    @Override
    public BedEnterProblem createObstructedProblem() {
        return new Problem(Component.translatable("block.minecraft.bed.obstructed"));
    }

    @Override
    public BedEnterProblem createNotSafeProblem() {
        return new Problem(Component.translatable("block.minecraft.bed.not_safe"));
    }

    @Override
    public BedEnterProblem createExplosionProblem() {
        return new Problem(null);
    }

    @Override
    public BedEnterProblem createOtherProblem() {
        return new Problem(null);
    }
}
