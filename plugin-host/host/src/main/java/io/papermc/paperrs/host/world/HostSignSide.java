package io.papermc.paperrs.host.world;

import com.google.common.base.Preconditions;
import io.papermc.paperrs.host.Components;
import io.papermc.paperrs.host.bridge.Values;
import java.util.ArrayList;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import net.kyori.adventure.text.Component;
import org.bukkit.DyeColor;
import org.bukkit.block.sign.SignSide;
import org.jetbrains.annotations.Nullable;

/** One side of a sign's snapshot ({@code CraftSignSide}). */
final class HostSignSide implements SignSide {
    private final HostBlockState state;
    private final String side;

    HostSignSide(final HostBlockState state, final String side) {
        this.state = state;
        this.side = side;
    }

    private Map<String, Object> text() {
        return Values.map(this.state.sign().get(this.side));
    }

    @SuppressWarnings("unchecked")
    private List<Object> rawLines() {
        return (List<Object>) this.text().get("lines");
    }

    @Override
    public List<Component> lines() {
        final List<Component> out = new ArrayList<>();
        for (final Object line : this.rawLines()) {
            final Component c = Components.fromJson(line);
            out.add(c == null ? Component.empty() : c);
        }
        return out;
    }

    @Override
    public Component line(final int index) {
        return this.lines().get(index);
    }

    @Override
    public void line(final int index, final Component line) {
        Preconditions.checkArgument(line != null, "line cannot be null");
        this.rawLines().set(index, Components.toJson(line));
        this.state.signChanged();
    }

    @Override
    public String[] getLines() {
        return this.lines().stream().map(Components::toLegacy).toArray(String[]::new);
    }

    @Override
    public String getLine(final int index) {
        return Components.toLegacy(this.line(index));
    }

    @Override
    public void setLine(final int index, final @Nullable String line) {
        this.line(index, line == null ? Component.empty() : Components.legacy(line));
    }

    @Override
    public boolean isGlowingText() {
        return Values.bool(this.text(), "glowing", false);
    }

    @Override
    public void setGlowingText(final boolean glowing) {
        this.text().put("glowing", glowing);
        this.state.signChanged();
    }

    @Override
    public @Nullable DyeColor getColor() {
        return DyeColor.valueOf(Values.str(this.text(), "color", "black").toUpperCase(Locale.ROOT));
    }

    @Override
    public void setColor(final @Nullable DyeColor color) {
        this.text().put("color", (color == null ? DyeColor.BLACK : color).name().toLowerCase(Locale.ROOT));
        this.state.signChanged();
    }
}
