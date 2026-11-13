package io.papermc.paperrs.test.paper;

import com.mojang.brigadier.arguments.IntegerArgumentType;
import io.papermc.paper.command.brigadier.Commands;
import io.papermc.paper.plugin.bootstrap.BootstrapContext;
import io.papermc.paper.plugin.bootstrap.PluginBootstrap;
import io.papermc.paper.plugin.lifecycle.event.types.LifecycleEvents;
import net.kyori.adventure.text.Component;

/** Registers a Brigadier command during bootstrap, before the server exists. */
public final class PaperTestBootstrap implements PluginBootstrap {
    @Override
    public void bootstrap(final BootstrapContext context) {
        context.getLifecycleManager().registerEventHandler(LifecycleEvents.COMMANDS, event -> {
            event.registrar().register(Commands.literal("dice")
                .then(Commands.argument("sides", IntegerArgumentType.integer(1, 100))
                    .executes(ctx -> {
                        final int sides = IntegerArgumentType.getInteger(ctx, "sides");
                        ctx.getSource().getSender().sendMessage(Component.text("You rolled a d" + sides));
                        return 1;
                    }))
                .executes(ctx -> {
                    ctx.getSource().getSender().sendMessage(Component.text("Usage: /dice <sides>"));
                    return 1;
                })
                .build(), "Rolls a die");
        });
    }
}
