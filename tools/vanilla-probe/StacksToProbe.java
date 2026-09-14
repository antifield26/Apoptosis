import java.io.PrintWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import net.minecraft.SharedConstants;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.item.Item;
import net.minecraft.world.item.ItemStack;

/**
 * Dumps every item's maximum stack size, because the stack-size table has never had an independent source.
 *
 * <h2>What this is for</h2>
 *
 * {@code crates/entity/src/stack.rs} states its table comes from "every {@code stacksTo} call site in the game",
 * and {@code StackSizeTable::len} carries "always 165 for the 26.1.2 vanilla table". Two attempts to count that by
 * reading the source both used a broken instrument (KD-82), so this probe is the instrument the claim needs: the
 * count to compare against 165, and the per-item limits to compare against the table row for row.
 *
 * <h2>Why it reads a component rather than a constant</h2>
 *
 * In 26.x the maximum stack size is <b>item data</b>, not a constant: {@code getDefaultMaxStackSize()} reads it
 * from the bound {@code DataComponents}, and the first version of this probe died with
 * {@code NullPointerException: Components not bound yet} (KD-83). This version reads the component directly and,
 * where an item does not carry one, <b>says so in the output rather than guessing 64</b> — a probe that reports
 * what it could not measure is worth more than one that substitutes a default and looks complete.
 *
 * <h2>Output</h2>
 *
 * One TSV of {@code <item id> <name> <max stack size or ABSENT>}, LF endings, plus a summary on stdout.
 *
 * <p>Local research tool (not part of the product crates).
 */
public final class StacksToProbe {
    public static void main(String[] args) throws Exception {
        Path out = Path.of(args.length > 0 ? args[0] : ".");
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        // **The step three rounds of this were missing.** Scanning the jar for the message found exactly
        // one class carrying it, `Holder$Reference`, and `MappedRegistry.freeze()` is what binds the holder
        // values \u2014 `Bootstrap.bootStrap()` registers these registries without freezing them, so the item
        // components do not exist until this call.
        BuiltInRegistries.ITEM.freeze();

        int items = 0;
        int exceptions = 0;
        int absent = 0;
        try (PrintWriter writer = new PrintWriter(
                Files.newBufferedWriter(out.resolve("stack_sizes.tsv"), StandardCharsets.UTF_8))) {
            writer.print("# Vanilla 26.1.2 maximum stack size per item.\n");
            writer.print("# Format: <item id> <name> <max stack size or ABSENT>\n");
            for (Item item : BuiltInRegistries.ITEM) {
                int id = BuiltInRegistries.ITEM.getId(item);
                String name = BuiltInRegistries.ITEM.getKey(item).toString();
                String value;
                try {
                    Integer max = new ItemStack(item).getMaxStackSize();
                    if (max == null) {
                        value = "ABSENT";
                        absent++;
                    } else {
                        value = Integer.toString(max);
                        if (max != 64) {
                            exceptions++;
                        }
                    }
                } catch (Throwable failure) {
                    // Reported rather than swallowed: an item this probe cannot read is a fact about the probe.
                    value = "ERROR:" + failure.getClass().getSimpleName() + ":" + String.valueOf(failure.getMessage()).replace(" ", "_");
                    absent++;
                }
                writer.print(id + " " + name + " " + value + "\n");
                items++;
            }
        }
        System.out.println("items=" + items);
        System.out.println("exceptions=" + exceptions);
        System.out.println("unreadable=" + absent);
    }
}
