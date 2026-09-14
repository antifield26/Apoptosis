import java.io.PrintWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import net.minecraft.SharedConstants;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.entity.EntityType;

/**
 * Dumps the entity-type registry in id order, for {@code entity_types.tsv}.
 *
 * <h2>Why this exists</h2>
 *
 * P10-06: the {@code add_entity} packet carries an <b>entity type id</b>, and the client resolves it against the
 * registry this server sends it. That is the same shape as the two defects this review already found and fixed:
 * a block's default state taken to be its lowest id (KD-56), and {@code PLAINS_BIOME_ID = 0} when id 0 in the
 * client's registry is {@code minecraft:badlands} (KD-65). <b>A number sent to a client is a claim about a
 * registry the client owns</b>, and the rule the review produced is that such a number needs a jar extraction, a
 * capture, or an assertion naming the registry — not a recollection.
 *
 * <p>So this is the jar extraction, in the pipeline {@code blocks.tsv} and {@code items.tsv} went through: a
 * probe boots the jar's own registry and writes what it finds, and the table is checked in rather than retyped.
 *
 * <h2>Output</h2>
 *
 * One TSV of {@code <entity type id> <name>}, LF endings, with the two header lines {@code blocks.tsv} and
 * {@code items.tsv} carry. Written with an explicit {@code "\n"} because {@code PrintWriter.println} emits CRLF
 * on Windows, which is how a CRLF table reached the repository once already.
 *
 * <p>Local research tool (not part of the product crates).
 */
public final class EntityTypeProbe {
    public static void main(String[] args) throws Exception {
        Path out = Path.of(args.length > 0 ? args[0] : ".");
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();

        int count = 0;
        try (PrintWriter writer = new PrintWriter(
                Files.newBufferedWriter(out.resolve("entity_types.tsv"), StandardCharsets.UTF_8))) {
            writer.print("# Vanilla 26.1.2 entity type registry order.\n");
            writer.print("# Format: <entity type id>\t<name>\n");
            for (EntityType<?> type : BuiltInRegistries.ENTITY_TYPE) {
                int id = BuiltInRegistries.ENTITY_TYPE.getId(type);
                String name = BuiltInRegistries.ENTITY_TYPE.getKey(type).toString();
                writer.print(id + "\t" + name + "\n");
                count++;
            }
        }
        System.out.println("entity_types=" + count);
        System.out.println("id 0 is: "
                + BuiltInRegistries.ENTITY_TYPE.getKey(BuiltInRegistries.ENTITY_TYPE.byId(0)));
        System.out.println("player is at id: "
                + BuiltInRegistries.ENTITY_TYPE.getId(
                        BuiltInRegistries.ENTITY_TYPE.getValue(
                                net.minecraft.resources.Identifier.parse("minecraft:player"))));
        System.out.println("item is at id: "
                + BuiltInRegistries.ENTITY_TYPE.getId(
                        BuiltInRegistries.ENTITY_TYPE.getValue(
                                net.minecraft.resources.Identifier.parse("minecraft:item"))));
    }
}
