import java.io.PrintWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import net.minecraft.SharedConstants;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.biome.Biome;

/**
 * Dumps the biome registry in id order, because the server sends a biome id it never checked.
 *
 * <h2>What this is for</h2>
 *
 * {@code crates/server/src/game.rs} fills every chunk's biome palette with
 *
 * <pre>{@code
 * // Biome ids are not modelled in P04: one plains biome fills every cell.
 * const PLAINS_BIOME_ID: u32 = 0;
 * }</pre>
 *
 * and the client resolves that number against the registry <b>this server sends it</b> — which is a verbatim
 * replay of the vanilla one. So the number only means "plains" if vanilla's registry happens to put plains at
 * id 0, and nothing in the repository checks that. It is the same shape as KD-56, where a block's default state
 * was assumed to be its lowest id and was wrong for 642 of 1168 blocks.
 *
 * A biome id decides the colour of grass, leaves and water, the sky and fog, and the mobs that may spawn — so a
 * world painted one biome looks wrong everywhere without anything erroring.
 *
 * <h2>Output</h2>
 *
 * One TSV of {@code <biome id> <name>}, LF endings, which is what the constant should be checked against.
 *
 * <p>Local research tool (not part of the product crates).
 */
public final class BiomeProbe {
    public static void main(String[] args) throws Exception {
        Path out = Path.of(args.length > 0 ? args[0] : ".");
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();

        int biomes = 0;
        try (PrintWriter writer = new PrintWriter(
                java.nio.file.Files.newBufferedWriter(
                        out.resolve("biomes.tsv"), StandardCharsets.UTF_8))) {
            writer.print("# Vanilla 26.1.2 biome registry order.\n");
            writer.print("# Format: <biome id> <name>\n");
            for (Biome biome : BuiltInRegistries.BIOME) {
                int id = BuiltInRegistries.BIOME.getId(biome);
                String name = BuiltInRegistries.BIOME.getKey(biome).toString();
                writer.print(id + " " + name + "\n");
                biomes++;
            }
        }
        System.out.println("biomes=" + biomes);
        System.out.println("id 0 is: " + BuiltInRegistries.BIOME.getKey(BuiltInRegistries.BIOME.byId(0)));
        System.out.println("plains is at id: "
                + BuiltInRegistries.BIOME.getId(BuiltInRegistries.BIOME.getValue(
                        net.minecraft.resources.ResourceLocation.parse("minecraft:plains"))));
    }
}
