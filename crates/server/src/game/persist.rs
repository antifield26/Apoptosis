//! Persistence: chunk/entity serialization, chunk loading, saves (P15-03).
//!
//! Mechanical split of `super`: every item here moved byte-identical.
//! No logic changed; `super` keeps the struct, the sessions and the tick.

use crate::storage::WorldService;
use mc_core::error::ServerResult;
use mc_entity::entity::{EntityBody, EntityKind};
use mc_entity::mob::MobKind;

use mc_nbt::NbtTag;
use mc_persistence::chunk::{ChunkData, ChunkPos};
use mc_persistence::dimension::Dimension;
use mc_world::chunk::Chunk;
use tracing::{debug, warn};

use super::{Game, chunk_of, nbt_int, to_entity};

impl Game {
    /// Give the owned world handle back (shutdown, or handing it to a save worker).
    ///
    /// Returns `None` when this game never owned one.
    #[must_use]
    pub fn into_storage(self) -> Option<WorldService> {
        self.storage
    }

    /// Mutable access to the owned world handle (autosave, admin tooling).
    pub fn storage_mut(&mut self) -> Option<&mut WorldService> {
        self.storage.as_mut()
    }

    /// Flush the owned world handle and close it.
    ///
    /// A no-op for a game that does not own storage. Afterwards the game has no
    /// storage: further chunk loads use the in-memory/placeholder path, and another
    /// save needs [`Game::save_all`] with a borrowed handle.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when a chunk cannot be encoded or `level.dat`
    /// cannot be written.
    pub fn close_storage(&mut self) -> ServerResult<()> {
        let Some(mut service) = self.storage.take() else {
            return Ok(());
        };
        // `queue_dirty_chunks` now reports how many placeholders it skipped, which
        // `close_storage` has no use for — the warning is emitted inside `save_all`, and a
        // shutdown path that also logged it would say the same thing twice.
        let result = self
            .queue_dirty_chunks(&mut service)
            .and_then(|_skipped| service.close().map(|_| ()));
        if let Err(error) = &result {
            warn!(%error, "closing the world failed");
        }
        result
    }

    /// Load a chunk into the world, from disk when one is stored.
    ///
    /// The order matters and is the reason this method exists at all:
    ///
    /// 1. already loaded → nothing to do;
    /// 2. stored on disk → convert and load it, then mark it **clean**. A chunk
    ///    that was loaded and not edited must never be written back: an all-air
    ///    placeholder saved over real terrain is the data loss this ordering
    ///    prevents;
    /// 3. nothing stored → create the all-air placeholder (generation is P07) and
    ///    mark it clean for the same reason — "we have no terrain for this" is not
    ///    the same as "this is empty terrain", and only a real edit makes it dirty;
    /// 4. read failed → placeholder, logged, and left clean: a failed read must
    ///    never license overwriting a file.
    ///
    /// Reads happen on the tick thread and stop at the per-tick chunk budget, so the
    /// worst case per tick is [`CHUNKS_PER_TICK`] chunk decodes. Moving them to a
    /// worker is P08-11 and would change the determinism story, not just the
    /// threading, so it is deliberately left on the tick thread here.
    pub(crate) fn load_or_create_chunk(&mut self, pos: ChunkPos) {
        if self.world.is_loaded(pos) {
            return;
        }
        let mut loaded = false;
        let mut read_failed = false;
        match self.read_stored_chunk(pos) {
            Ok(Some(data)) => {
                let entities = data.entities.clone();
                let block_entities = data.block_entities.clone();
                match Chunk::from_chunk_data(&data, &self.registries.blocks) {
                    Ok(chunk) => {
                        self.world.load_chunk(chunk);
                        loaded = true;
                        // P11-08: the chunk's saved entities come back as live
                        // entities; the Broadcast phase announces them like any
                        // other spawn. P12-05: block entities ride the same path.
                        self.load_chunk_entities(&entities);
                        self.load_chunk_block_entities(&block_entities);
                    }
                    Err(error) => {
                        warn!(?pos, %error, "stored chunk could not be converted; using a placeholder");
                        read_failed = true;
                    }
                }
            }
            Ok(None) => {}
            Err(error) => {
                warn!(?pos, %error, "chunk read failed; using a placeholder (it will not be saved)");
                read_failed = true;
            }
        }
        if read_failed {
            // AUDIT-09 B-01: a chunk that is *stored but unreadable* must never
            // be generated over. The loaded terrain would be clean, so the file
            // would survive until the first edit made the chunk dirty and the
            // next autosave replaced real terrain with generated blocks — the
            // data-loss class the generation gate exists to prevent. The mark
            // lasts for the session; the next boot re-reads the file.
            self.unreadable_chunks.insert(pos);
        }
        if !loaded {
            // Generation requires knowing that **nothing is stored**, and only a game that
            // owns storage can know that: `read_stored_chunk` returns `Ok(None)` both for
            // "no chunk here" and for "I have no handle to look with". Treating the second
            // as the first would generate terrain over a saved world, which is unrecoverable
            // — so a borrowing game keeps the placeholder and its do-not-persist mark
            // instead. Found by the worldgen E2E test; see the fix's commit message.
            //
            // This is the **only** place generation happens, which is what makes "an
            // existing world is never regenerated" a structural property rather than a
            // promise.
            let may_generate =
                self.can_read_stored_chunks() && !self.unreadable_chunks.contains(&pos);
            match self
                .generator
                .as_ref()
                .filter(|_| may_generate)
                .map(|generator| {
                    // The trait must be in scope for the method to resolve; naming it here
                    // rather than at the top of the file keeps the import next to its only use.
                    use mc_worldgen::ChunkGenerator as _;
                    generator
                        .generate_chunk(pos, &self.registries.blocks)
                        .map_err(|error| error.to_string())
                }) {
                Some(Ok(mut chunk)) => {
                    // Left **clean**, which is the non-obvious part: this generator is
                    // deterministic, so a generated chunk is reproducible byte for byte
                    // from `(seed, pos)` at any later time. Persisting it buys nothing, and
                    // a dirty chunk cannot be unloaded — so marking it dirty would make
                    // generated chunks accumulate forever. What must be persisted is a
                    // *modification*, and `set_block` marks the chunk dirty for that.
                    //
                    // An earlier version of this path marked it dirty "so the world keeps
                    // it", which broke chunk unloading for every generated chunk and (via a
                    // matching change to the clean-marking below) let placeholders be
                    // written back over real terrain. Two existing tests caught both.
                    //
                    // Structures decorate the terrain, so they run **after** it and read the same
                    // height field the terrain pass used. A refusal is counted rather than fatal: a
                    // missing decoration is strictly better than losing the terrain a player stands
                    // on.
                    self.decorate_with_structures(pos, &mut chunk);
                    // **The oak features.** `generate_chunk` is terrain only — `decorate` is a separate pass,
                    // exactly as vanilla separates them — and **nothing called it**, so every world this
                    // server generated had no trees in it at all. The biome surface blocks were right (grass
                    // over dirt over stone, podzol and coarse dirt for taiga, sand and water for ocean), which
                    // is why the world read as terrain stripped of its features rather than as broken terrain.
                    //
                    // Last, so a tree is not planted through a structure placed a line earlier.
                    if self.generator.is_some() {
                        // Rebuilt from the seed for the same reason `decorate_with_structures` rebuilds its
                        // context: one source of truth for the seed, and no borrow of `self` held across the
                        // mutable use of `chunk`.
                        let context = mc_worldgen::WorldgenContext::overworld(
                            mc_worldgen::WorldSeed::from_raw(self.random_seed),
                        );
                        match mc_worldgen::TerrainGenerator::new(context, &self.registries.blocks) {
                            Ok(generator) => {
                                let stats =
                                    generator.decorate(&mut chunk, pos, &self.registries.blocks);
                                self.tree_stats.record(stats);
                            }
                            Err(error) => {
                                warn!(?pos, %error, "a chunk could not be decorated with trees");
                            }
                        }
                    }
                    self.world.load_chunk(chunk);
                }
                Some(Err(error)) => {
                    warn!(?pos, %error, "chunk generation failed; using a placeholder");
                    self.world.ensure_chunk(pos);
                    if !self.can_read_stored_chunks() {
                        self.placeholder_without_storage.insert(pos);
                    }
                }
                None => {
                    // No generator, or a game that cannot tell "absent" from "unreadable":
                    // a placeholder is only safe to *persist* when this game could have read
                    // the real chunk and found nothing. Without storage it cannot know, so it
                    // keeps the placeholder but refuses to let it be written back by marking
                    // it "not saved yet" rather than clean.
                    self.world.ensure_chunk(pos);
                    if !may_generate {
                        self.placeholder_without_storage.insert(pos);
                    }
                }
            }
        }
        // Every path ends clean, and the three origins reach that state for three
        // different reasons — which is why the line is unconditional:
        //
        //   * a **stored** chunk is unmodified;
        //   * a **generated** chunk is reproducible from the seed;
        //   * a **placeholder** is a stand-in, and `ensure_chunk` marks it dirty by
        //     construction, so without this it would be written back over real terrain.
        //
        // The third is the one an earlier version broke by making this conditional, which
        // `a_stored_chunk_is_loaded_from_disk_and_never_overwritten_by_a_placeholder` caught.
        self.mark_chunk_clean(pos);
    }

    /// The saved entities of one chunk as NBT (P11-08).
    ///
    /// The shape is vanilla-compatible for the fields a client or server needs
    /// to reconstruct the entity: `id` (the resource id), `Pos` (three
    /// doubles), `Motion` (three doubles), and for a mob `Health`; for a
    /// dropped item, `Item` with `id` and `Count`. Fields this build does not
    /// model are absent, so a vanilla server reading our file would see a
    /// default-valued entity rather than a corrupt one — the honest direction
    /// for a gap.
    fn serialize_chunk_entities(&self, pos: ChunkPos) -> Vec<mc_nbt::NbtTag> {
        self.entities
            .iter()
            .filter(|entity| {
                !entity.removed
                    && entity.kind() != EntityKind::Player
                    && chunk_of(entity.position.x, entity.position.z) == pos
            })
            .map(|entity| {
                let mut fields: Vec<(String, mc_nbt::NbtTag)> = Vec::new();
                match &entity.body {
                    EntityBody::Mob(mob) => {
                        fields.push((
                            "id".to_owned(),
                            mc_nbt::NbtTag::String(format!("minecraft:{}", mob.kind.name())),
                        ));
                        fields.push(("Health".to_owned(), mc_nbt::NbtTag::Float(entity.health)));
                    }
                    EntityBody::Item(item) => {
                        let item_name = item
                            .item_id()
                            .and_then(|id| self.registries.items.name(id).ok())
                            .unwrap_or_default();
                        fields.push((
                            "id".to_owned(),
                            mc_nbt::NbtTag::String("minecraft:item".to_owned()),
                        ));
                        fields.push((
                            "Item".to_owned(),
                            mc_nbt::NbtTag::Compound(vec![
                                (
                                    "id".to_owned(),
                                    mc_nbt::NbtTag::String(item_name.to_owned()),
                                ),
                                ("Count".to_owned(), mc_nbt::NbtTag::Int(item.stack.count())),
                            ]),
                        ));
                    }
                    EntityBody::Player | EntityBody::Projectile(_) => {}
                }
                fields.push((
                    "Pos".to_owned(),
                    mc_nbt::NbtTag::List(vec![
                        mc_nbt::NbtTag::Double(entity.position.x),
                        mc_nbt::NbtTag::Double(entity.position.y),
                        mc_nbt::NbtTag::Double(entity.position.z),
                    ]),
                ));
                fields.push((
                    "Motion".to_owned(),
                    mc_nbt::NbtTag::List(vec![
                        mc_nbt::NbtTag::Double(entity.velocity.x),
                        mc_nbt::NbtTag::Double(entity.velocity.y),
                        mc_nbt::NbtTag::Double(entity.velocity.z),
                    ]),
                ));
                mc_nbt::NbtTag::Compound(fields)
            })
            .collect()
    }

    /// Spawn the entities a saved chunk carries (P11-08).
    ///
    /// Malformed entries are logged and skipped: one bad entity must not stop
    /// the chunk from loading, and the loss is visible in the log. Every skip
    /// below carries its own reason at `warn!` for that second half — an
    /// earlier version of this function documented "logged and skipped" while
    /// several of its `continue`s were silent, which is the failure mode where a
    /// save quietly loses entities and nothing anywhere says so.
    fn load_chunk_entities(&mut self, tags: &[mc_nbt::NbtTag]) {
        for tag in tags {
            let mc_nbt::NbtTag::Compound(fields) = tag else {
                warn!("a saved entity was not a compound; skipped");
                continue;
            };
            let get = |name: &str| {
                fields
                    .iter()
                    .find(|(key, _)| key == name)
                    .map(|(_, value)| value)
            };
            let Some(mc_nbt::NbtTag::String(id)) = get("id") else {
                warn!("a saved entity has no string `id`; skipped");
                continue;
            };
            let position = match get("Pos") {
                Some(mc_nbt::NbtTag::List(coords)) if coords.len() == 3 => {
                    let (
                        Some(mc_nbt::NbtTag::Double(x)),
                        Some(mc_nbt::NbtTag::Double(y)),
                        Some(mc_nbt::NbtTag::Double(z)),
                    ) = (coords.first(), coords.get(1), coords.get(2))
                    else {
                        warn!(%id, "a saved entity's `Pos` is not three doubles; skipped");
                        continue;
                    };
                    mc_world::Vec3::new(*x, *y, *z)
                }
                _ => {
                    warn!(%id, "a saved entity has no three-element `Pos`; skipped");
                    continue;
                }
            };
            let kind = MobKind::from_name(id.strip_prefix("minecraft:").unwrap_or(id));
            if id == "minecraft:item" {
                let Some(mc_nbt::NbtTag::Compound(item_fields)) = get("Item") else {
                    warn!("a saved item has no `Item` compound; skipped");
                    continue;
                };
                let (Some(NbtTag::String(item_name)), Some(NbtTag::Int(count))) = (
                    item_fields.iter().find(|(k, _)| k == "id").map(|(_, v)| v),
                    item_fields
                        .iter()
                        .find(|(k, _)| k == "Count")
                        .map(|(_, v)| v),
                ) else {
                    warn!("a saved item has no `Item.id`/`Item.Count` pair; skipped");
                    continue;
                };
                let Ok(item_id) = self.registries.items.id(item_name) else {
                    warn!(item = %item_name, "a saved item names an unknown item; skipped");
                    continue;
                };
                // `ItemStack::new` takes `(item_id, count)`. Passing them the
                // other way round produced a stack of whatever item had the
                // *count* as its registry id -- a saved diamond pickaxe came
                // back as N of item 1 -- and the argument order is the whole
                // reason this line is called out (found by the P11-08
                // persistence test).
                let Ok(stack) = mc_entity::stack::ItemStack::new(item_id, *count) else {
                    warn!("a saved item's stack is not a legal stack; skipped");
                    continue;
                };
                if self.spawn_item(stack, position).is_err() {
                    warn!("a saved item could not be spawned");
                }
                continue;
            }
            let Some(kind) = kind else {
                warn!(%id, "a saved entity names a kind this build does not model; skipped");
                continue;
            };
            if self.spawn_mob(kind, to_entity(position)).is_err() {
                warn!("a saved mob could not be spawned");
            }
        }
    }

    /// The saved block entities of one chunk as NBT (P12-05).
    ///
    /// Shape: `id` (`minecraft:chest`/`minecraft:furnace`/`minecraft:hopper`),
    /// `x`/`y`/`z` ints, `Items` list of `{Slot byte, id string, Count int}`,
    /// plus furnace `BurnTicks`/`BurnTotal`/`CookProgress`/`CookTotal` ints and
    /// hopper `Cooldown` int. Vanilla reads `id`/`x`/`y`/`z`/`Items` and ignores
    /// the rest, so a vanilla boot sees chests with contents and furnaces with
    /// items but reset progress — the honest direction for the progress gap.
    fn serialize_chunk_block_entities(&self, pos: ChunkPos) -> Vec<mc_nbt::NbtTag> {
        self.block_entities
            .iter()
            .filter(|entity| {
                let (ex, ez) = (entity.pos.x >> 4, entity.pos.z >> 4);
                ex == pos.x && ez == pos.z
            })
            .map(|entity| {
                let mut fields: Vec<(String, mc_nbt::NbtTag)> = Vec::new();
                let id = match entity.kind() {
                    mc_container::BlockEntityKind::Container => "minecraft:chest",
                    mc_container::BlockEntityKind::Furnace => "minecraft:furnace",
                    mc_container::BlockEntityKind::Hopper => "minecraft:hopper",
                    mc_container::BlockEntityKind::Sign => "minecraft:sign",
                };
                fields.push(("id".to_owned(), mc_nbt::NbtTag::String(id.to_owned())));
                fields.push(("x".to_owned(), mc_nbt::NbtTag::Int(entity.pos.x)));
                fields.push(("y".to_owned(), mc_nbt::NbtTag::Int(entity.pos.y)));
                fields.push(("z".to_owned(), mc_nbt::NbtTag::Int(entity.pos.z)));
                if let Some(items) = entity.data.items() {
                    let list = items
                        .iter()
                        .enumerate()
                        .filter(|(_, s)| !s.is_empty())
                        .filter_map(|(slot, stack)| {
                            let name = stack
                                .item_id()
                                .and_then(|item_id| self.registries.items.name(item_id).ok())?;
                            let byte_slot = i8::try_from(slot).ok()?;
                            Some(mc_nbt::NbtTag::Compound(vec![
                                ("Slot".to_owned(), mc_nbt::NbtTag::Byte(byte_slot)),
                                ("id".to_owned(), mc_nbt::NbtTag::String(name.to_owned())),
                                ("Count".to_owned(), mc_nbt::NbtTag::Int(stack.count())),
                            ]))
                        })
                        .collect();
                    fields.push(("Items".to_owned(), mc_nbt::NbtTag::List(list)));
                }
                match &entity.data {
                    mc_container::BlockEntityData::Furnace {
                        burn_ticks,
                        burn_total,
                        cook_progress,
                        cook_total,
                        ..
                    } => {
                        fields.push((
                            "BurnTicks".to_owned(),
                            mc_nbt::NbtTag::Int(*burn_ticks as i32),
                        ));
                        fields.push((
                            "BurnTotal".to_owned(),
                            mc_nbt::NbtTag::Int(*burn_total as i32),
                        ));
                        fields.push((
                            "CookProgress".to_owned(),
                            mc_nbt::NbtTag::Int(*cook_progress as i32),
                        ));
                        fields.push((
                            "CookTotal".to_owned(),
                            mc_nbt::NbtTag::Int(*cook_total as i32),
                        ));
                    }
                    mc_container::BlockEntityData::Hopper { cooldown, .. } => {
                        fields.push(("Cooldown".to_owned(), mc_nbt::NbtTag::Int(*cooldown as i32)));
                    }
                    _ => {}
                }
                mc_nbt::NbtTag::Compound(fields)
            })
            .collect()
    }

    /// Restore the block entities a saved chunk carries (P12-05).
    ///
    /// Malformed entries are logged and skipped like entities: one bad chest
    /// must not stop the chunk. Signs are skipped (no payload this build
    /// persists for them).
    fn load_chunk_block_entities(&mut self, tags: &[mc_nbt::NbtTag]) {
        for tag in tags {
            let mc_nbt::NbtTag::Compound(fields) = tag else {
                warn!("a saved block entity was not a compound; skipped");
                continue;
            };
            let get = |name: &str| {
                fields
                    .iter()
                    .find(|(key, _)| key == name)
                    .map(|(_, value)| value)
            };
            let Some(mc_nbt::NbtTag::String(id)) = get("id") else {
                warn!("a saved block entity has no string `id`; skipped");
                continue;
            };
            let (Some(x), Some(y), Some(z)) =
                (nbt_int(get("x")), nbt_int(get("y")), nbt_int(get("z")))
            else {
                warn!(%id, "a saved block entity has no int x/y/z; skipped");
                continue;
            };
            let pos = mc_container::BlockPos::new(x, y, z);
            let kind = match id.as_str() {
                "minecraft:chest" | "minecraft:trapped_chest" | "minecraft:barrel" => {
                    mc_container::BlockEntityKind::Container
                }
                "minecraft:furnace" => mc_container::BlockEntityKind::Furnace,
                "minecraft:hopper" => mc_container::BlockEntityKind::Hopper,
                _ => {
                    warn!(%id, "a saved block entity names an unmodelled kind; skipped");
                    continue;
                }
            };
            let mut entity = mc_container::BlockEntity::new(pos, kind);
            // Items list, tolerant of Byte/Short/Int slots and counts.
            if let Some(mc_nbt::NbtTag::List(entries)) = get("Items")
                && let Some(items) = entity.data.items_mut()
            {
                for entry in entries {
                    let mc_nbt::NbtTag::Compound(entry_fields) = entry else {
                        continue;
                    };
                    let find = |name: &str| {
                        entry_fields
                            .iter()
                            .find(|(key, _)| key == name)
                            .map(|(_, value)| value)
                    };
                    let (Some(slot), Some(item_name), Some(count)) = (
                        find("Slot").and_then(|t| nbt_int(Some(t))),
                        match find("id") {
                            Some(mc_nbt::NbtTag::String(name)) => Some(name),
                            _ => None,
                        },
                        find("Count").and_then(|t| nbt_int(Some(t))),
                    ) else {
                        continue;
                    };
                    let Ok(slot) = usize::try_from(slot) else {
                        continue;
                    };
                    let Ok(item_id) = self.registries.items.id(item_name) else {
                        warn!(item = %item_name, "a saved block item is unknown; skipped");
                        continue;
                    };
                    let Ok(stack) = mc_entity::stack::ItemStack::new(item_id, count) else {
                        continue;
                    };
                    if slot < items.len() && !stack.is_empty() {
                        items[slot] = stack;
                    }
                }
            }
            match &mut entity.data {
                mc_container::BlockEntityData::Furnace {
                    burn_ticks,
                    burn_total,
                    cook_progress,
                    cook_total,
                    ..
                } => {
                    *burn_ticks = nbt_int(get("BurnTicks")).map_or(0, |v| v.max(0) as u32);
                    *burn_total = nbt_int(get("BurnTotal")).map_or(0, |v| v.max(0) as u32);
                    *cook_progress = nbt_int(get("CookProgress")).map_or(0, |v| v.max(0) as u32);
                    *cook_total = nbt_int(get("CookTotal")).map_or(0, |v| v.max(0) as u32);
                }
                mc_container::BlockEntityData::Hopper { cooldown, .. } => {
                    *cooldown = nbt_int(get("Cooldown")).map_or(0, |v| v.max(0) as u32);
                }
                _ => {}
            }
            self.block_entities.insert(entity);
        }
    }

    /// Place whatever structure this chunk's selection picks, if any.
    ///
    /// A no-op when no templates are loaded, so a server without a data pack pays nothing. The
    /// selection is a pure function of `(seed, chunk_pos)`, so a chunk decorated here and one decorated
    /// after a restart get the same structure — which is what makes regeneration reproducible.
    fn decorate_with_structures(&mut self, pos: ChunkPos, chunk: &mut Chunk) {
        if self.structures.is_empty() {
            return;
        }
        self.structure_stats.considered += 1;
        if self.generator.is_none() {
            // No generator means no terrain, and a structure on a placeholder would be decoration on
            // nothing. Skipped rather than placed.
            return;
        }
        // Rebuilt from the seed rather than stored: `WorldgenContext::overworld` is the only
        // constructor the server uses, and deriving it here keeps one source of truth for the seed.
        let context = mc_worldgen::WorldgenContext::overworld(mc_worldgen::WorldSeed::from_raw(
            self.random_seed,
        ));
        let surface_height = |x: i32, z: i32| {
            self.generator
                .as_ref()
                .map_or(0, |generator| generator.surface_height(x, z))
        };
        let request = mc_worldgen::structures::StructureGenRequest {
            pos,
            context: &context,
            registry: &self.structures,
            set: &self.structure_set,
            build: mc_worldgen::structures::StructureBuild::default(),
            blocks: &self.registries.blocks,
            surface_height: &surface_height,
        };
        let report = mc_worldgen::structures::generate_structures(chunk, &request);
        if report.selected {
            self.structure_stats.selected += 1;
            self.structure_stats.blocks_written += report.blocks_written;
            if report.refused.is_some() {
                self.structure_stats.refused += 1;
            }
        }
        if report.selected && report.blocks_written == 0 {
            // Selected but wrote nothing: either it was refused or every block was air under
            // `IgnoreAir`. Logged because it is the difference between "no structure here" and
            // "a structure that did not appear", which is otherwise invisible.
            debug!(
                ?pos,
                name = ?report.name,
                outside = report.blocks_outside,
                out_of_world = report.blocks_out_of_world,
                refused = ?report.refused,
                "a structure was selected but wrote no blocks"
            );
        }
    }

    /// Load a chunk, generating or reading it as appropriate.
    ///
    /// The same path the chunk streamer uses, exposed because it is genuinely useful
    /// outside it: a test needs to place a chunk deterministically, and a future
    /// `/forceload` needs to load one that no player is near. It is deliberately **not** a
    /// test-only shortcut — a second loading path would be a second place for the
    /// "prefer stored over generated" ordering to be got wrong.
    ///
    /// Returns whether a chunk is loaded afterwards, which is `false` only when generation
    /// failed *and* storage could not supply one.
    pub fn load_chunk(&mut self, pos: ChunkPos) -> bool {
        self.load_or_create_chunk(pos);
        self.world.is_loaded(pos)
    }

    /// Read a chunk from the owned storage, when there is one.
    ///
    /// `Ok(None)` means "nothing is stored here", which is different from an error
    /// and is what decides between a real load and a placeholder.
    ///
    /// # Errors
    ///
    /// Propagates the storage layer's error so the caller can log it. The caller
    /// degrades to a placeholder; it never fails the tick.
    fn read_stored_chunk(&mut self, pos: ChunkPos) -> ServerResult<Option<ChunkData>> {
        let Some(storage) = self.storage.as_mut() else {
            return Ok(None);
        };
        storage.storage_mut().read_chunk(&Dimension::Overworld, pos)
    }

    fn mark_chunk_clean(&mut self, pos: ChunkPos) {
        if let Some(chunk) = self.world.chunk_mut(pos) {
            chunk.mark_clean();
        }
    }

    // ---------------------------------------------------------------- sending

    /// Persist the world through a caller-supplied handle (shutdown path).
    ///
    /// Persists: **dirty** chunks (terrain plus their live entities and block
    /// entities, P11-08/P12-05) and `level.dat`. Does **not** persist per-player
    /// data —`playerdata/<uuid>.dat` needs the player-file layout, which Phase 04
    /// does not implement. Only chunks the world reports as dirty are written,
    /// so an entity in an otherwise-clean chunk still does not save (recorded
    /// divergence).
    ///
    /// Only chunks the world reports as dirty are written. A chunk that was loaded
    /// from disk and then edited is dirty; a chunk that was only *streamed* to a
    /// player is not, which is what stops an all-air placeholder from overwriting
    /// real terrain.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when a chunk cannot be encoded or the flush
    /// fails.
    pub fn save_all(&mut self, storage: &mut WorldService) -> ServerResult<()> {
        let skipped = self.queue_dirty_chunks(storage)?;
        if skipped > 0 {
            // Reported rather than silent: a save that deliberately left chunks out is
            // different from one that saved everything, and an operator should be able to
            // tell which happened.
            warn!(
                skipped,
                "placeholder chunks were not saved: this game cannot read storage, so it \
                 cannot tell an empty chunk from an unreadable one"
            );
        }
        let report = storage.storage_mut().flush()?;
        if report.is_clean() {
            self.world.clear_dirty();
        } else {
            // Keep the dirty flags. Clearing them here would forget a failed write
            // entirely: the chunk would never be retried and the loss would show up
            // only as missing terrain after a restart (Audit 03).
            warn!(
                failed = report.chunks_failed,
                errors = ?report.errors,
                "world flush reported failures; dirty chunks keep their flags for retry"
            );
        }
        Ok(())
    }

    /// [`Game::save_all`] against the handle this game owns.
    ///
    /// **Silently does nothing when this game does not own storage**, and returns `Ok(())`
    /// when it does so. That is deliberate — a game built with `Game::new` or
    /// `Game::with_seed` holds a *borrow* and legitimately has no handle to flush, and
    /// [`crate::lifecycle::Server::run`] calls this on every autosave tick regardless —
    /// but it is a sharp edge worth naming: a caller who pairs it with a borrowing
    /// constructor gets a save that reports success and writes nothing, and the loss only
    /// appears as missing terrain after a restart. Use [`Game::save_all`] with the service
    /// you borrowed from instead.
    ///
    /// # Errors
    ///
    /// As for [`Game::save_all`].
    pub fn save_all_owned(&mut self) -> ServerResult<()> {
        let Some(mut service) = self.storage.take() else {
            return Ok(());
        };
        let result = self.save_all(&mut service);
        self.storage = Some(service);
        result
    }

    /// Encode and queue every dirty chunk, **except** the do-not-persist placeholders.
    ///
    /// The skip is the whole point of [`Game::placeholder_without_storage`], and until this
    /// was written nothing consulted that set: the field was populated on every placeholder
    /// path and read by nobody, so a placeholder was still written over real terrain. A
    /// generated world made it visible — the borrowing-game test placed a marker and watched
    /// it vanish — but the defect predates generation, and the doc comment on the field was
    /// more confident than the code.
    fn queue_dirty_chunks(&self, storage: &mut WorldService) -> ServerResult<usize> {
        let mut skipped = 0usize;
        for pos in self.world.dirty_chunks() {
            if self.placeholder_without_storage.contains(&pos) {
                // Belt to the clean-flag's braces. A placeholder is cleaned when it is
                // created, so a placeholder should never *be* dirty — but the consequence of
                // being wrong here is destroying a saved world, so the check stays and is
                // reported. (Before this check existed, the field was populated on every
                // placeholder path and read by nobody, which is how a placeholder once
                // overwrote real terrain.)
                skipped += 1;
                continue;
            }
            if let Some(chunk) = self.world.chunk(pos) {
                let mut data = chunk.to_chunk_data(&self.registries.blocks)?;
                // P11-08: the chunk's live entities ride the save, so a
                // restart puts the world back the way it was left. P12-05: the
                // block entities ride the same save.
                data.entities = self.serialize_chunk_entities(pos);
                data.block_entities = self.serialize_chunk_block_entities(pos);
                storage
                    .storage_mut()
                    .queue_chunk_save(&Dimension::Overworld, &data)?;
            }
        }
        Ok(skipped)
    }
}
