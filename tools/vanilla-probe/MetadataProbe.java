import java.io.PrintWriter;
import java.lang.reflect.Field;
import java.lang.reflect.Modifier;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.network.syncher.EntityDataAccessor;
import net.minecraft.network.syncher.EntityDataSerializer;
import net.minecraft.network.syncher.EntityDataSerializers;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.entity.EntityType;

/**
 * Dumps the per-entity-type metadata index tables, which is P10-07's extraction.
 *
 * <h2>Why this probe is not like the others</h2>
 *
 * Every previous table in this project came out of a registry: iterate it, ask each entry for its id, write a row.
 * **Entity metadata is not in a registry.** Vanilla defines it with {@code SynchedEntityData.defineId(...)} calls
 * inside each entity class, so the table lives in the <b>bytecode</b>, one static {@code EntityDataAccessor} field
 * per metadata slot, and each accessor carries both its index and its serializer.
 *
 * <p>So this walks the classes. For every registered entity type it takes the type's class, forces it to
 * initialise — which is what runs the {@code defineId} calls — and reads the static {@code EntityDataAccessor}
 * fields by reflection. Each accessor answers {@code id()} for the slot and {@code serializer()} for the wire
 * type, which is exactly the pair the {@code set_entity_data} codec needs and cannot derive.
 *
 * <h2>Why the capture is not enough on its own</h2>
 *
 * The capture in {@code target/vanilla-capture/bodies-lit/} has 45 {@code set_entity_data} bodies, and two of them
 * show {@code index 16} carrying a byte in one and a VarInt in the other. **A table written from those two bodies
 * would be a fixture and a decoder agreeing because they came from one reading** — the failure mode this phase
 * exists to find. The index and the type have to come from the jar, and the capture is then what checks them.
 *
 * <h2>Output</h2>
 *
 * One TSV of {@code <entity type name> <index> <serializer class>}, LF endings, sorted by type then index.
 *
 * <p>Local research tool (not part of the product crates).
 */
public final class MetadataProbe {
    public static void main(String[] args) throws Exception {
        Path out = Path.of(args.length > 0 ? args[0] : ".");
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();

        int types = 0;
        int slots = 0;
        int skipped = 0;
        try (PrintWriter writer = new PrintWriter(
                Files.newBufferedWriter(out.resolve("entity_metadata.tsv"), StandardCharsets.UTF_8))) {
            writer.print("# Vanilla 26.1.2 entity metadata slots, read from the classes.\n");
            writer.print("# Format: <entity type>\t<index>\t<serializer>\n");
            for (EntityType<?> type : BuiltInRegistries.ENTITY_TYPE) {
                String name = BuiltInRegistries.ENTITY_TYPE.getKey(type).toString();
                Class<?> clazz = type.getBaseClass();
                List<String> rows = new ArrayList<>();
                try {
                    // Forcing initialisation is what runs the `defineId` calls in the static initialiser.
                    Class.forName(clazz.getName(), true, clazz.getClassLoader());
                    slots += collect(clazz, rows);
                } catch (Throwable failure) {
                    // Named rather than swallowed: a class this probe cannot read is a fact about the probe, and
                    // the row says so instead of leaving a gap that looks like "this type has no metadata".
                    rows.add("ERROR\t" + failure.getClass().getSimpleName());
                    skipped++;
                }
                for (String row : rows) {
                    writer.print(name + "\t" + row + "\n");
                }
                types++;
            }
        }
        System.out.println("types=" + types);
        System.out.println("slots=" + slots);
        System.out.println("unreadable=" + skipped);
    }

    /**
     * The wire id and name of a serializer, e.g. {@code 3 (FLOAT)}.
     *
     * Found by **identity** against the named constants of {@code EntityDataSerializers}, whose declaration order
     * is the wire order: the capture decodes {@code type 3} as a float and {@code type 0} as a byte, which is
     * {@code FLOAT} and {@code BYTE}. Comparing objects rather than class names also survives two serializers
     * sharing an implementation class, which is exactly what made the first version of this column useless.
     */
    private static String serializerId(EntityDataSerializer<?> serializer) {
        int index = 0;
        for (Field field : EntityDataSerializers.class.getDeclaredFields()) {
            if (!Modifier.isStatic(field.getModifiers())
                    || !EntityDataSerializer.class.isAssignableFrom(field.getType())) {
                continue;
            }
            try {
                field.setAccessible(true);
                if (field.get(null) == serializer) {
                    return index + " (" + field.getName() + ")";
                }
            } catch (Throwable ignored) {
                // A field this cannot read is counted rather than skipped silently; see the caller.
            }
            index++;
        }
        return "UNKNOWN";
    }

    /** Every static {@code EntityDataAccessor} in this class and its superclasses, as `index<TAB>serializer`. */
    private static int collect(Class<?> clazz, List<String> rows) throws Exception {
        int found = 0;
        for (Class<?> at = clazz; at != null && at != Object.class; at = at.getSuperclass()) {
            for (Field field : at.getDeclaredFields()) {
                if (!Modifier.isStatic(field.getModifiers())) {
                    continue;
                }
                if (!EntityDataAccessor.class.isAssignableFrom(field.getType())) {
                    continue;
                }
                field.setAccessible(true);
                Object value = field.get(null);
                if (!(value instanceof EntityDataAccessor<?> accessor)) {
                    continue;
                }
                rows.add(accessor.id() + "\t" + serializerId(accessor.serializer()));
                found++;
            }
        }
        return found;
    }
}
