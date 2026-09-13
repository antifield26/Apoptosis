import java.io.PrintWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import net.minecraft.SharedConstants;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.item.BlockItem;
import net.minecraft.world.item.Item;

/**
 * Dumps the item registry in id order, so {@code items.tsv} can be checked against the jar rather than trusted.
 *
 * <h2>Why</h2>
 *
 * Every other registry table in this repository records what made it: {@code blocks.tsv} names
 * {@code DumpRegistries} and its compaction script, {@code block_light.tsv} names {@code LightProbe}, and the
 * anvil fixtures carry a manifest with the jar's sha1 and a hash per file. **{@code items.tsv} carries only a
 * claim** — "Vanilla 26.1.2 item registry order" — and nothing that would fail if it were wrong.
 *
 * That is the shape of every confirmed failure of the P00-P09 review round: an expectation whose source is the
 * same understanding as the code it checks, so the two agree whether or not either is right. A table of 1 400
 * item ids is exactly where a transcription error would be invisible — every lookup would still succeed, and
 * would name the wrong item.
 *
 * <h2>Output</h2>
 *
 * The same three columns {@code items.tsv} claims: {@code <item id> <name> <block or ->}, tab-separated, LF
 * endings. Diffing the two is the check.
 *
 * <p>Local research tool (not part of the product crates).
 */
public final class ItemProbe {
    public static void main(String[] args) throws Exception {
        Path out = Path.of(args.length > 0 ? args[0] : ".");
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();

        int items = 0;
        try (PrintWriter writer = new PrintWriter(
                java.nio.file.Files.newBufferedWriter(
                        out.resolve("items_probe.tsv"), StandardCharsets.UTF_8))) {
            writer.print("# Vanilla 26.1.2 item registry order.\n");
            writer.print("# Format: <item id>\t<name>\t<block or ->\n");
            for (Item item : BuiltInRegistries.ITEM) {
                int id = BuiltInRegistries.ITEM.getId(item);
                String name = BuiltInRegistries.ITEM.getKey(item).toString();
                String block = item instanceof BlockItem blockItem
                        ? BuiltInRegistries.BLOCK.getKey(blockItem.getBlock()).toString()
                        : "-";
                writer.print(id + "\t" + name + "\t" + block + "\n");
                items++;
            }
        }
        System.out.println("items=" + items);
    }
}
