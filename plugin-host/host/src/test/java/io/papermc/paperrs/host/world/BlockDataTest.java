package io.papermc.paperrs.host.world;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.junit.jupiter.api.Assertions.fail;

import io.papermc.paperrs.host.Unsupported;
import java.lang.reflect.Field;
import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;
import java.util.ArrayList;
import java.util.List;
import java.util.Locale;
import java.util.Set;
import org.bukkit.Instrument;
import org.bukkit.Note;
import org.bukkit.block.BlockFace;
import org.bukkit.block.BlockType;
import org.bukkit.block.data.Ageable;
import org.bukkit.block.data.Bisected;
import org.bukkit.block.data.BlockData;
import org.bukkit.block.data.Rotatable;
import org.bukkit.block.data.type.Door;
import org.bukkit.block.data.type.Fence;
import org.bukkit.block.data.type.NoteBlock;
import org.bukkit.block.data.type.Snow;
import org.bukkit.block.data.type.Stairs;
import org.junit.jupiter.api.Test;

/**
 * Block data proxies map API accessors onto block properties by name, which the compiler
 * cannot check. So this calls every getter of every block's data interface, and every setter
 * with the value its getter returned, for all blocks.
 */
class BlockDataTest {
    private static List<String> blocks() {
        final List<String> out = new ArrayList<>();
        for (final Field field : BlockType.class.getFields()) {
            if (Modifier.isStatic(field.getModifiers()) && BlockType.class.isAssignableFrom(field.getType())) {
                out.add("minecraft:" + field.getName().toLowerCase(Locale.ROOT));
            }
        }
        return out;
    }

    private static Object call(final Method method, final Object target, final Object... args) throws Throwable {
        try {
            return method.invoke(target, args);
        } catch (InvocationTargetException e) {
            throw e.getCause();
        }
    }

    /** A sample argument for an accessor keyed by a face or a slot number. */
    private static Object key(final Class<?> type) {
        return type == BlockFace.class ? BlockFace.NORTH : type == int.class ? (Object) 0 : null;
    }

    @Test
    void everyAccessorOfEveryBlockWorks() {
        final List<String> problems = new ArrayList<>();
        final List<String> blocks = blocks();
        assertTrue(blocks.size() > 1000, "found " + blocks.size() + " block types");
        for (final String block : blocks) {
            final BlockData data = HostBlockData.defaults(block);
            final Class<? extends BlockData> api = HostBlockData.dataClass(block);
            if (!api.isInstance(data)) {
                problems.add(block + " is not a " + api.getSimpleName());
                continue;
            }
            if (!HostBlockData.parse(null, data.getAsString()).equals(data)) {
                problems.add(block + " does not survive getAsString: " + data.getAsString());
            }
            for (final Method method : api.getMethods()) {
                if (method.getDeclaringClass() == BlockData.class || Modifier.isStatic(method.getModifiers())) {
                    continue;
                }
                final Class<?>[] params = method.getParameterTypes();
                try {
                    if (method.getReturnType() != void.class) {
                        if (params.length == 0) {
                            call(method, data);
                        } else if (params.length == 1) {
                            call(method, data, key(params[0]));
                        }
                        continue;
                    }
                    if (!method.getName().startsWith("set") || params.length == 0 || params.length > 2) {
                        continue;
                    }
                    // Set what the getter says: it must be accepted and change nothing.
                    final Object[] keys = params.length == 2 ? new Object[] {key(params[0])} : new Object[0];
                    final Class<?> value = params[params.length - 1];
                    final String rest = method.getName().substring(3);
                    Method get = null;
                    for (final Method m : api.getMethods()) {
                        final String n = m.getName();
                        final boolean named = n.equals("get" + rest) || n.equals("is" + rest) || n.equals("has" + rest)
                            || rest.startsWith("Has") && n.equals("has" + rest.substring(3));
                        if (named && m.getParameterCount() == keys.length && m.getReturnType() == value) {
                            get = m;
                        }
                    }
                    if (get == null) {
                        problems.add(block + ": no getter for " + method.getName());
                        continue;
                    }
                    final BlockData copy = data.clone();
                    final Object[] args = new Object[keys.length + 1];
                    System.arraycopy(keys, 0, args, 0, keys.length);
                    args[keys.length] = call(get, copy, keys);
                    call(method, copy, args);
                    // (Deprecated conveniences such as CreakingHeart#setActive may pick another value.)
                    if (!copy.equals(data) && !method.isAnnotationPresent(Deprecated.class)) {
                        problems.add(block + ": " + method.getName() + "(" + get.getName() + "()) changed it to " + copy.getAsString());
                    }
                } catch (Throwable t) {
                    problems.add(block + ": " + api.getSimpleName() + "#" + method.getName() + " threw " + t);
                }
            }
        }
        if (!problems.isEmpty()) {
            fail(problems.size() + " problems:\n" + String.join("\n", problems.subList(0, Math.min(40, problems.size()))));
        }
    }

    @Test
    void parsingMatchingAndMerging() {
        final BlockData east = HostBlockData.parse(null, "minecraft:oak_stairs[facing=east]");
        final Stairs stairs = (Stairs) east;
        assertEquals(BlockFace.EAST, stairs.getFacing());
        assertEquals(Bisected.Half.BOTTOM, stairs.getHalf());
        assertEquals(Stairs.Shape.STRAIGHT, stairs.getShape());
        assertEquals("minecraft:oak_stairs[facing=east]", east.getAsString(true));
        assertEquals("minecraft:oak_stairs[facing=east,half=bottom,shape=straight,waterlogged=false]", east.getAsString());
        assertEquals(east, HostBlockData.parse(null, "oak_stairs[facing=east]"));

        final Stairs top = (Stairs) HostBlockData.defaults("minecraft:oak_stairs");
        top.setHalf(Bisected.Half.TOP);
        top.setFacing(BlockFace.EAST);
        assertTrue(top.matches(east), "only the given properties count");
        assertFalse(top.matches(HostBlockData.parse(null, "minecraft:oak_stairs[facing=west]")));
        assertFalse(top.matches(HostBlockData.defaults("minecraft:stone")));
        final Stairs merged = (Stairs) HostBlockData.defaults("minecraft:oak_stairs").merge(east);
        assertEquals(BlockFace.EAST, merged.getFacing());

        final BlockData clone = top.clone();
        ((Stairs) clone).setFacing(BlockFace.NORTH);
        assertEquals(BlockFace.EAST, top.getFacing(), "clones are independent");
        assertNotEquals(top, clone);

        assertThrows(IllegalArgumentException.class, () -> HostBlockData.parse(null, "minecraft:oak_stairs[facing=up]"));
        assertThrows(IllegalArgumentException.class, () -> HostBlockData.parse(null, "minecraft:no_such_block"));
        assertThrows(IllegalArgumentException.class, () -> top.setFacing(BlockFace.UP));
        assertThrows(IllegalArgumentException.class, () -> ((Ageable) HostBlockData.defaults("minecraft:wheat")).setAge(8));
    }

    @Test
    void irregularProperties() {
        final Door door = (Door) HostBlockData.defaults("minecraft:oak_door");
        door.setHalf(Bisected.Half.TOP);
        assertTrue(door.getAsString().contains("half=upper"), door.getAsString());

        final NoteBlock note = (NoteBlock) HostBlockData.defaults("minecraft:note_block");
        assertEquals(Instrument.PIANO, note.getInstrument());
        note.setInstrument(Instrument.BASS_DRUM);
        note.setNote(new Note(12));
        assertTrue(note.getAsString().contains("instrument=basedrum") && note.getAsString().contains("note=12"), note.getAsString());

        final Rotatable sign = (Rotatable) HostBlockData.parse(null, "minecraft:oak_sign[rotation=4]");
        assertEquals(BlockFace.WEST, sign.getRotation());
        sign.setRotation(BlockFace.EAST);
        assertTrue(sign.getAsString().contains("rotation=12"));

        final Snow snow = (Snow) HostBlockData.defaults("minecraft:snow");
        assertEquals(1, snow.getMinimumLayers());
        assertEquals(8, snow.getMaximumLayers());

        final Fence fence = (Fence) HostBlockData.defaults("minecraft:oak_fence");
        assertTrue(fence.getFaces().isEmpty());
        assertEquals(Set.of(BlockFace.NORTH, BlockFace.EAST, BlockFace.SOUTH, BlockFace.WEST), fence.getAllowedFaces());
        fence.setFace(BlockFace.NORTH, true);
        assertTrue(fence.hasFace(BlockFace.NORTH));
        assertEquals(Set.of(BlockFace.NORTH), fence.getFaces());

        // Methods about the block's behaviour say what is missing.
        final Unsupported unsupported = assertThrows(Unsupported.class, fence::getSoundGroup);
        assertTrue(unsupported.getMessage().contains("BlockData#getSoundGroup"), unsupported.getMessage());
    }
}
