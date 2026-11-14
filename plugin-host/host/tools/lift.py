#!/usr/bin/env python3
"""Copies Paper's server-side classes that the plugin host reuses.

Paper's plugin loading pipeline (io.papermc.paper.plugin), its Bukkit scheduler, the
console sender and a few helpers do not depend on Minecraft itself. Rather than rewrite
them, the host compiles copies of them from Paper's paper-server sources.

This script re-creates src/lifted/java from a checkout of Paper
(https://github.com/PaperMC/Paper): it copies the files listed below from
paper-server/src/main/java, stamps each with its origin, and applies the few edits recorded
in EDITS (each must match exactly once, so an upstream change that invalidates an edit fails
loudly instead of being silently skipped). Run it again after updating to a newer Paper.

    python3 -I plugin-host/host/tools/lift.py /path/to/Paper
"""
import pathlib
import shutil
import sys

HERE = pathlib.Path(__file__).resolve().parent
HOST = HERE.parent
REPO = HOST.parent.parent
PAPER = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 else REPO.parent / 'Paper'
SRC = PAPER / 'paper-server' / 'src' / 'main' / 'java'
DST = HOST / 'src' / 'lifted' / 'java'

# Whole directories, minus files the host replaces with its own version.
TREES = {
    'io/papermc/paper/plugin': {
        # rewritten in src/main/java: drives scanning without the dedicated server
        'io/papermc/paper/plugin/PluginInitializerManager.java',
        # rewritten: the original fires tag events through Minecraft's tag loader
        'io/papermc/paper/plugin/lifecycle/event/types/PaperTagEventTypeProvider.java',
    },
    'org/bukkit/craftbukkit/metadata': set(),
    'org/bukkit/craftbukkit/scheduler': set(),
    # Commodore's rerouting: enum-to-registry compatibility and its annotations
    'org/bukkit/craftbukkit/legacy/enums': set(),
    'org/bukkit/craftbukkit/legacy/reroute': set(),
}

FILES = [
    'org/bukkit/craftbukkit/util/ApiVersion.java',
    # Commodore: rewrites plugin classes as they load (renamed fields, Material.values()...)
    'org/bukkit/craftbukkit/util/Commodore.java',
    'org/bukkit/craftbukkit/util/ClassTraverser.java',
    'org/bukkit/craftbukkit/util/CraftLegacy.java',
    'org/bukkit/craftbukkit/legacy/FieldRename.java',
    'org/bukkit/craftbukkit/legacy/MethodRerouting.java',
    'org/bukkit/craftbukkit/legacy/fieldrename/FieldRenameData.java',
    'org/bukkit/craftbukkit/util/Versioning.java',
    'org/bukkit/craftbukkit/util/Waitable.java',
    'org/bukkit/craftbukkit/util/WeakCollection.java',
    'org/bukkit/craftbukkit/util/permissions/CraftDefaultPermissions.java',
    'org/bukkit/craftbukkit/util/permissions/CommandPermissions.java',
    'org/bukkit/craftbukkit/command/ServerCommandSender.java',
    'org/bukkit/craftbukkit/command/CraftConsoleCommandSender.java',
    'org/bukkit/craftbukkit/conversations/ConversationTracker.java',
    'com/destroystokyo/paper/ServerSchedulerReportingWrapper.java',
    'io/papermc/paper/threadedregions/scheduler/FoliaAsyncScheduler.java',
    'io/papermc/paper/threadedregions/scheduler/FallbackRegionScheduler.java',
    'io/papermc/paper/util/StackWalkerUtil.java',
    'io/papermc/paper/persistence/PaperPersistentDataKey.java',
    'io/papermc/paper/adventure/providers/MiniMessageProviderImpl.java',
    'io/papermc/paper/adventure/providers/AdventurePropertiesDefaultOverrideProviderImpl.java',
    'io/papermc/paper/configuration/constraint/Constraint.java',
    'io/papermc/paper/configuration/serializer/ComponentSerializer.java',
    'io/papermc/paper/configuration/serializer/EnumValueSerializer.java',
]

# (file, exact text, replacement, why)
EDITS = [
    ('org/bukkit/craftbukkit/util/Commodore.java',
     'import joptsimple.OptionParser;\nimport joptsimple.OptionSet;\nimport joptsimple.OptionSpec;\n', '',
     'the command-line converter (main) is left out: the host does not bundle jopt-simple'),
    ('org/bukkit/craftbukkit/util/Commodore.java',
     '''    public static void main(String[] args) {
        OptionParser parser = new OptionParser();''',
     '''    private static void unusedMain(String[] args) {
        if (true) throw new UnsupportedOperationException();''',
     'see above'),
    ('org/bukkit/craftbukkit/util/Commodore.java',
     '''        OptionSpec<File> inputFlag = parser.acceptsAll(Arrays.asList("i", "input")).withRequiredArg().ofType(File.class).required();
        OptionSpec<File> outputFlag = parser.acceptsAll(Arrays.asList("o", "output")).withRequiredArg().ofType(File.class).required();

        OptionSet options = parser.parse(args);

        File input = options.valueOf(inputFlag);
        File output = options.valueOf(outputFlag);
''',
     '''        File input = new File(args[0]);
        File output = new File(args[1]);
''',
     'see above'),
    ('org/bukkit/craftbukkit/util/Commodore.java',
     'import org.bukkit.craftbukkit.legacy.MaterialRerouting;\n', '',
     'MaterialRerouting (Material arguments of plugins without an api-version) needs CraftBukkit\'s item and tag classes; the host does not convert such plugins'),
    ('org/bukkit/craftbukkit/util/Commodore.java',
     '''                .create(compatibilityPresent)
                .forClass(MaterialRerouting.class)
                .build();''',
     '''                .create(compatibilityPresent)
                .build();''',
     'see above'),
    ('io/papermc/paper/plugin/manager/PaperPluginInstanceManager.java',
     'import io.papermc.paper.FeatureHooks;\n', '',
     'FeatureHooks flushes the server\'s async log appenders, which the host does not have'),
    ('io/papermc/paper/plugin/manager/PaperPluginInstanceManager.java',
     '            FeatureHooks.flushAsyncAppenders();\n', '',
     'see above'),
    ('io/papermc/paper/plugin/manager/PaperPluginInstanceManager.java',
     'import org.bukkit.craftbukkit.util.CraftMagicNumbers;\n', '',
     'CraftMagicNumbers is Minecraft-bound'),
    ('io/papermc/paper/plugin/manager/PaperPluginInstanceManager.java',
     'CraftMagicNumbers.isLegacy(descriptionFile)', 'descriptionFile.getAPIVersion() == null',
     'the same test without CraftMagicNumbers: a plugin without api-version is legacy'),
    ('io/papermc/paper/plugin/provider/source/FileProviderSource.java',
     'import io.papermc.paper.SparksFly;\n', '',
     'the host does not bundle the spark profiler'),
    ('io/papermc/paper/plugin/provider/source/FileProviderSource.java',
     '''        if ((config.getName().equals("spark") && config.getMainClass().equals("me.lucko.spark.bukkit.BukkitSparkPlugin")) && !SparksFly.isPluginPreferred()) {
            LOGGER.info("The spark plugin will not be loaded as this server bundles the spark profiler.");
            return;
        }
''', '',
     'see above'),
    ('org/bukkit/craftbukkit/util/Versioning.java',
     'import net.minecraft.SharedConstants;\n', '',
     'Minecraft-bound fallback'),
    ('org/bukkit/craftbukkit/util/Versioning.java',
     'apiVersion = SharedConstants.getCurrentVersion().id();',
     'apiVersion = io.papermc.paperrs.host.HostVersions.MINECRAFT_VERSION;',
     'use the host\'s version constant'),
    ('org/bukkit/craftbukkit/command/CraftConsoleCommandSender.java',
     'return io.papermc.paper.configuration.GlobalConfiguration.get().console.hasAllPermissions || super.hasPermission(name);',
     'return io.papermc.paperrs.host.HostVersions.CONSOLE_HAS_ALL_PERMISSIONS || super.hasPermission(name);',
     'Paper\'s global configuration is not loaded by the host'),
    ('org/bukkit/craftbukkit/command/CraftConsoleCommandSender.java',
     'return io.papermc.paper.configuration.GlobalConfiguration.get().console.hasAllPermissions || super.hasPermission(perm);',
     'return io.papermc.paperrs.host.HostVersions.CONSOLE_HAS_ALL_PERMISSIONS || super.hasPermission(perm);',
     'see above'),
]


def main() -> int:
    if not SRC.is_dir():
        print(f'paper-server sources not found at {SRC}; pass the path of a Paper checkout', file=sys.stderr)
        return 2
    if DST.exists():
        shutil.rmtree(DST)
    copied = []
    for tree, skip in TREES.items():
        for path in sorted((SRC / tree).rglob('*.java')):
            rel = path.relative_to(SRC).as_posix()
            if rel not in skip:
                copied.append(rel)
    copied.extend(FILES)

    edits_by_file = {}
    for rel, old, new, why in EDITS:
        edits_by_file.setdefault(rel, []).append((old, new, why))

    for rel in copied:
        text = (SRC / rel).read_text(encoding='utf-8')
        applied = []
        for old, new, why in edits_by_file.pop(rel, []):
            count = text.count(old)
            if count != 1:
                print(f'{rel}: expected exactly one match for an edit ({why}), found {count}', file=sys.stderr)
                return 1
            text = text.replace(old, new)
            applied.append(why)
        header = f'// Copied from paper-server/src/main/java/{rel} by plugin-host/host/tools/lift.py.\n'
        if applied:
            header += '// Edited: ' + '; '.join(dict.fromkeys(applied)) + '.\n'
        out = DST / rel
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(header + text, encoding='utf-8')

    if edits_by_file:
        print(f'edits for files that were not copied: {sorted(edits_by_file)}', file=sys.stderr)
        return 1
    print(f'copied {len(copied)} files into {DST.relative_to(REPO)}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
