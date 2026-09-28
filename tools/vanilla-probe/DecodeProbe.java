import io.netty.buffer.Unpooled;
import net.minecraft.SharedConstants;
import net.minecraft.core.component.DataComponentPatch;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.network.RegistryFriendlyByteBuf;
import net.minecraft.server.Bootstrap;

/**
 * Decodes a raw component-patch body through vanilla 26.1.2's own
 * {@code DataComponentPatch} stream codec (client jar), isolating the patch
 * framing from the item-default bootstrap limit that blocks full-stack
 * decodes in a bare harness ("Components not bound yet").
 *
 * <p>Proves the patch order fix: {@code added, removed, entries…} decodes to
 * {damage=1, max_damage=59}; the old order ({@code added, entries…,
 * removed}) throws inside the patch codec.
 *
 * <p>Local research tool (not part of the product crates).
 */
public final class DecodeProbe {
    static void decodePatch(String label, byte[] patch) {
        RegistryFriendlyByteBuf buf = new RegistryFriendlyByteBuf(
                Unpooled.wrappedBuffer(patch),
                net.minecraft.core.RegistryAccess.fromRegistryOfRegistries(
                        BuiltInRegistries.REGISTRY));
        try {
            DataComponentPatch decoded = DataComponentPatch.STREAM_CODEC.decode(buf);
            System.out.println(label + ": PATCH OK (" + decoded
                    + "), trailing=" + buf.readableBytes());
        } catch (Throwable t) {
            System.out.println(label + ": PATCH FAILED: " + t);
        }
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();

        // Fixed order (what the server sends now).
        decodePatch("fixed ",
                new byte[] {0x02, 0x00, 0x03, 0x01, 0x02, 0x3b});
        // Old order (what the server sent during the walk).
        decodePatch("old   ",
                new byte[] {0x02, 0x03, 0x01, 0x02, 0x3b, 0x00});
        // Empty patch (both orders coincide).
        decodePatch("empty ",
                new byte[] {0x00, 0x00});
    }
}
