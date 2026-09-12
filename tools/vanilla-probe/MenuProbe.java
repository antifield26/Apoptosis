import java.lang.reflect.Field;
import java.util.List;

/**
 * Dumps the player-inventory menu's slot layout from the official 26.1.2 server
 * jar, so the container permutation in mc-entity is evidence rather than recall.
 *
 * Prints, for the menu the client sees as window id 0, each menu slot index and
 * the backing container slot it maps to.
 */
public class MenuProbe {
    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion();
        net.minecraft.server.Bootstrap.bootStrap();

        // The player's own inventory menu.
        net.minecraft.world.entity.player.Inventory inventory =
                new net.minecraft.world.entity.player.Inventory(
                        (net.minecraft.world.entity.player.Player) null);

        net.minecraft.world.inventory.InventoryMenu menu =
                new net.minecraft.world.inventory.InventoryMenu(0, inventory);

        List<net.minecraft.world.inventory.Slot> slots = menu.slots;
        System.out.println("InventoryMenu slot count = " + slots.size());
        for (int i = 0; i < slots.size(); i++) {
            net.minecraft.world.inventory.Slot slot = slots.get(i);
            int containerSlot = slot.getContainerSlot();
            String name = slot.getClass().getSimpleName();
            System.out.println("menu[" + i + "] -> containerSlot=" + containerSlot
                    + " class=" + name
                    + " x=" + slot.x + " y=" + slot.y);
        }

        // Which container slot each armour position actually is, by asking the
        // Inventory what it holds for slots 36..40.
        System.out.println("--- armour container slots ---");
        for (int i = 36; i <= 40; i++) {
            net.minecraft.world.item.ItemStack stack = inventory.getItem(i);
            System.out.println("inventory[" + i + "] item=" + stack);
        }
    }
}
