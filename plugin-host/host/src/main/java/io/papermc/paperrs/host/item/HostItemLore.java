package io.papermc.paperrs.host.item;

import java.util.ArrayList;
import java.util.List;
import net.kyori.adventure.text.Component;
import net.kyori.adventure.text.ComponentLike;
import net.kyori.adventure.text.format.NamedTextColor;
import net.kyori.adventure.text.format.Style;
import net.kyori.adventure.text.format.TextDecoration;

/** A {@code minecraft:lore} value: the lines under an item's name. */
public record HostItemLore(List<Component> lines) implements io.papermc.paper.datacomponent.item.ItemLore {
    /** How the client shows lore lines that set no style of their own (vanilla's LORE_STYLE). */
    private static final Style LORE_STYLE = Style.style(NamedTextColor.DARK_PURPLE, TextDecoration.ITALIC);

    public HostItemLore {
        lines = List.copyOf(lines);
    }

    @Override
    public List<Component> styledLines() {
        return this.lines.stream().map(line -> line.applyFallbackStyle(LORE_STYLE)).toList();
    }

    /** {@code ItemLore.Builder}: at most 256 lines, as vanilla allows. */
    public static final class Builder implements io.papermc.paper.datacomponent.item.ItemLore.Builder {
        private final List<Component> lines = new ArrayList<>();

        @Override
        public Builder lines(final List<? extends ComponentLike> lines) {
            this.lines.clear();
            return this.addLines(lines);
        }

        @Override
        public Builder addLine(final ComponentLike line) {
            if (this.lines.size() >= 256) {
                throw new IllegalArgumentException("Cannot have more than 256 lines, had " + (this.lines.size() + 1));
            }
            this.lines.add(line.asComponent());
            return this;
        }

        @Override
        public Builder addLines(final List<? extends ComponentLike> lines) {
            lines.forEach(this::addLine);
            return this;
        }

        @Override
        public HostItemLore build() {
            return new HostItemLore(this.lines);
        }
    }
}
