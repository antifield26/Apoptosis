import java.io.PrintWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import net.minecraft.SharedConstants;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.item.Item;

/**
 * Dumps every item's maximum stack size, because the stack-size table has never had an independent source.
 *
 * <h2>What this is for</h2>
 *
 * {@code crates/entity/src/stack.rs} states its table comes from "every {@code stacksTo} call site in the game",
 * and {@code StackSizeTable::len} carries
 *
 * <pre>{@code
 * /// Number of exception entries (always 165 for the 26.1.2 vanilla table).
 * }</pre>
 *
 * <p>Two attempts to count that by reading the source both used a broken instrument — a regex that found neither
 * array, and a range extraction that returned the same number for two different arrays. <b>This probe is the
 * instrument the claim needs</b>: the count to compare against 165, and the per-item limits to compare against
 * the table row for row.
 *
 * <p>It is the same kind of check that made {@code items.tsv} trustworthy — {@code ItemProbe} reproduced all
 * 1506 of its rows — and the stack-size table is the last of these tables without one. <b>A doc saying where a
 * number came from is not the same as the number having been checked.</b>
 *
 * <h2>Output</h2>
 *
 * One TSV of {@code <item id> <name> <max stack size>}, LF endings.
 *
 * <p>Local research tool (not part of the product crates).
 */
public final class StacksToProbe {
    public static void main(String[] args) throws Exception {
        Path out = Path.of(args.length > 0 ? args[0] : ".");
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();

        int items = 0;
        int exceptions = 0;
        try (PrintWriter writer = new PrintWriter(
                Files.newBufferedWriter(out.resolve("stack_sizes.tsv"), StandardCharsets.UTF_8))) {
            writer.print("# Vanilla 26.1.2 maximum stack size per item.\n");
            writer.print("# Format: <item id> <name> <max stack size>\n");
            for (Item item : BuiltInRegistries.ITEM) {
                int id = BuiltInRegistries.ITEM.getId(item);
                String name = BuiltInRegistries.ITEM.getKey(item).toString();
                int max = item.getDefaultMaxStackSize();
                writer.print(id + " " + name + " " + max + "\n");
                items++;
                if (max != 64) {
                    exceptions++;
                }
            }
        }
        System.out.println("items=" + items);
        System.out.println("exceptions=" + exceptions);
    }
}
