package io.papermc.paperrs.host.item;

import java.util.ArrayList;
import java.util.List;
import org.bukkit.Color;

/** A {@code minecraft:custom_model_data} value: the floats, flags, strings and colours item models select by. */
public record HostCustomModelData(List<Float> floats, List<Boolean> flags, List<String> strings, List<Color> colors) implements io.papermc.paper.datacomponent.item.CustomModelData {
    public HostCustomModelData {
        floats = List.copyOf(floats);
        flags = List.copyOf(flags);
        strings = List.copyOf(strings);
        colors = List.copyOf(colors);
    }

    static HostCustomModelData of(final ItemComponents.CustomModelData data) {
        return new HostCustomModelData(data.floats(), data.flags(), data.strings(), data.colors().stream().map(c -> Color.fromRGB(c & 0xFFFFFF)).toList());
    }

    ItemComponents.CustomModelData toComponent() {
        return new ItemComponents.CustomModelData(this.floats, this.flags, this.strings, this.colors.stream().map(Color::asRGB).toList());
    }

    /** {@code CustomModelData.Builder}. */
    public static final class Builder implements io.papermc.paper.datacomponent.item.CustomModelData.Builder {
        private final List<Float> floats = new ArrayList<>();
        private final List<Boolean> flags = new ArrayList<>();
        private final List<String> strings = new ArrayList<>();
        private final List<Color> colors = new ArrayList<>();

        @Override
        public Builder addFloat(final float value) {
            this.floats.add(value);
            return this;
        }

        @Override
        public Builder addFloats(final List<Float> values) {
            values.forEach(this::addFloat);
            return this;
        }

        @Override
        public Builder addFlag(final boolean value) {
            this.flags.add(value);
            return this;
        }

        @Override
        public Builder addFlags(final List<Boolean> values) {
            values.forEach(this::addFlag);
            return this;
        }

        @Override
        public Builder addString(final String value) {
            this.strings.add(java.util.Objects.requireNonNull(value, "string"));
            return this;
        }

        @Override
        public Builder addStrings(final List<String> values) {
            values.forEach(this::addString);
            return this;
        }

        @Override
        public Builder addColor(final Color value) {
            this.colors.add(java.util.Objects.requireNonNull(value, "color"));
            return this;
        }

        @Override
        public Builder addColors(final List<Color> values) {
            values.forEach(this::addColor);
            return this;
        }

        @Override
        public HostCustomModelData build() {
            return new HostCustomModelData(this.floats, this.flags, this.strings, this.colors);
        }
    }
}
