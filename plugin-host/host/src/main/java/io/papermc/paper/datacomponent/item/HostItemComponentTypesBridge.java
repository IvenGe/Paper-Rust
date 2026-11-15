package io.papermc.paper.datacomponent.item;

import io.papermc.paperrs.host.item.HostCustomModelData;
import io.papermc.paperrs.host.item.HostItemLore;

/**
 * Paper's builders of data component values ({@code CustomModelData.customModelData()},
 * {@code ItemLore.lore()}...), for the components the host supports; the others fail naming
 * the builder. In Paper's package because the bridge is package-private there.
 */
public final class HostItemComponentTypesBridge extends AbstractItemComponentTypesBridge {
    @Override
    public CustomModelData.Builder customModelData() {
        return new HostCustomModelData.Builder();
    }

    @Override
    public ItemLore.Builder lore() {
        return new HostItemLore.Builder();
    }
}
