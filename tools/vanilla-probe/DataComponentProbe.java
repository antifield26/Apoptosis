import java.io.PrintWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import net.minecraft.SharedConstants;
import net.minecraft.core.component.DataComponentType;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;

/**
 * Dumps the data-component-type registry in id order, so the wire type ids
 * in {@code mc-entity/src/components.rs} ({@code TYPE_DAMAGE = 3}, …) can be
 * checked against the jar rather than trusted.
 *
 * <h2>Output</h2>
 *
 * {@code <type id> <name>}, tab-separated, LF endings. Diffing the rows this
 * project sends against this table is the check.
 *
 * <p>Local research tool (not part of the product crates).
 */
public final class DataComponentProbe {
    public static void main(String[] args) throws Exception {
        Path out = Path.of(args.length > 0 ? args[0] : ".");
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();

        int count = 0;
        try (PrintWriter writer = new PrintWriter(
                java.nio.file.Files.newBufferedWriter(
                        out.resolve("data_components_probe.tsv"), StandardCharsets.UTF_8))) {
            writer.print("# Vanilla 26.1.2 data component type registry order.\n");
            writer.print("# Format: <type id>\t<name>\n");
            for (DataComponentType<?> type : BuiltInRegistries.DATA_COMPONENT_TYPE) {
                int id = BuiltInRegistries.DATA_COMPONENT_TYPE.getId(type);
                String name =
                        BuiltInRegistries.DATA_COMPONENT_TYPE.getKey(type).toString();
                writer.print(id + "\t" + name + "\n");
                count++;
            }
        }
        System.out.println("components=" + count);
    }
}
