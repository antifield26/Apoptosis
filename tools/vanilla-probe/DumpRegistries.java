import java.io.BufferedWriter;
import java.io.PrintWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.List;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.Property;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;

/**
 * Dumps the vanilla 26.1.2 block-state id table and item registry to TSV.
 *
 * Method: boot the server's own registry (`Bootstrap.bootStrap()`, the same call
 * the dedicated server makes) and walk `Block.BLOCK_STATE_REGISTRY`, whose index
 * IS the id sent on the wire in `level_chunk_with_light` and `block_update`.
 * Nothing is guessed: this is the registry the client is built against.
 *
 * Local research tool (not part of the product crates).
 */
public final class DumpRegistries {
    public static void main(String[] args) throws Exception {
        Path out = Path.of(args.length > 0 ? args[0] : ".");
        // The dedicated server does exactly this pair before touching any
        // registry: detect the bundled version (version.json on the classpath),
        // then bootstrap the built-in registries.
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();

        List<BlockState> states = new ArrayList<>();
        for (BlockState state : Block.BLOCK_STATE_REGISTRY) {
            states.add(state);
        }

        // Block state ids: index in the registry, name + sorted property values.
        try (PrintWriter w = writer(out.resolve("block_states.tsv"))) {
            w.println("# Vanilla 26.1.2 block-state ids, from Block.BLOCK_STATE_REGISTRY.");
            w.println("# Format: <id>\\t<block name>\\t<properties k=v,k=v or ->");
            for (int id = 0; id < states.size(); id++) {
                BlockState state = states.get(id);
                StringBuilder props = new StringBuilder();
                for (Property<?> property : state.getProperties()) {
                    if (props.length() > 0) {
                        props.append(',');
                    }
                    props.append(property.getName()).append('=').append(value(state, property));
                }
                w.println(id + "\t" + BuiltInRegistries.BLOCK.getKey(state.getBlock())
                        + "\t" + (props.length() == 0 ? "-" : props));
            }
        }

        // Blocks: registry order (used for item ids / default states).
        try (PrintWriter w = writer(out.resolve("blocks.tsv"))) {
            w.println("# Vanilla 26.1.2 block registry order.");
            w.println("# Format: <block id>\\t<name>\\t<default state id>\\t<state count>");
            int blockId = 0;
            for (Block block : BuiltInRegistries.BLOCK) {
                w.println(blockId + "\t" + BuiltInRegistries.BLOCK.getKey(block)
                        + "\t" + Block.getId(block.defaultBlockState())
                        + "\t" + block.getStateDefinition().getPossibleStates().size());
                blockId++;
            }
        }

        // Items: registry order + the block they place (if any).
        // Stack size is deliberately not read here: `ItemStack` needs the data
        // component map, which the dedicated server binds after this point.
        try (PrintWriter w = writer(out.resolve("items.tsv"))) {
            w.println("# Vanilla 26.1.2 item registry order.");
            w.println("# Format: <item id>\t<name>\t<block or ->");
            int itemId = 0;
            for (var item : BuiltInRegistries.ITEM) {
                var key = BuiltInRegistries.ITEM.getKey(item);
                var block = item instanceof net.minecraft.world.item.BlockItem blockItem
                        ? BuiltInRegistries.BLOCK.getKey(blockItem.getBlock()).toString()
                        : "-";
                w.println(itemId + "\t" + key + "\t" + block);
                itemId++;
            }
        }

        // The registries the configuration phase sends, for cross-checking.
        try (PrintWriter w = writer(out.resolve("registries.tsv"))) {
            w.println("# Vanilla 26.1.2 built-in registry contents (names only).");
            w.println("# Format: <registry>\t<id>\t<entry>");
            for (var registry : BuiltInRegistries.REGISTRY) {
                Object registryKey = registry.key();
                w.println("# " + registryKey + " size=" + registry.size());
                int id = 0;
                for (Object entry : registry) {
                    w.println(registryKey + "\t" + id + "\t" + entry);
                    id++;
                }
            }
        }

        System.out.println("block states: " + states.size());
        System.out.println("blocks: " + BuiltInRegistries.BLOCK.size());
        System.out.println("items: " + BuiltInRegistries.ITEM.size());
    }

    private static <T extends Comparable<T>> String value(BlockState state, Property<T> property) {
        return property.getName(state.getValue(property));
    }

    private static PrintWriter writer(Path path) throws Exception {
        return new PrintWriter(new BufferedWriter(Files.newBufferedWriter(path, StandardCharsets.UTF_8)));
    }

    private DumpRegistries() {}
}
