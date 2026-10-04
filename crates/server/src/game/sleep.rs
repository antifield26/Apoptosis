//! Beds and sleep, slice 1: placement, sleep attempts, skip-night (P20-04).
//!
//! ## Jar truth (26.1.2, `javap -c` on the server jar, read 2026-10-02)
//!
//! - `ServerPlayer.startSleepInBed`: already sleeping or dead →
//!   `OTHER_PROBLEM`; `BedRule` (dimension attribute — always sleepable and
//!   spawnable in the single overworld; the nether explosion lives in
//!   `BedBlock.useWithoutItem`, P21) → `bedInRange` (clicked half or the
//!   opposite-facing neighbour within |dx|≤3, |dy|≤2, |dz|≤3 of the player)
//!   else `TOO_FAR_AWAY` → `bedBlocked` (both cells above foot and head free)
//!   else `OBSTRUCTED` → set respawn when spawnable → `canSleep` else the
//!   rule's problem → non-creative monster check (any `Monster` in the
//!   16×10×16 box around the bed's bottom centre) else `NOT_SAFE` → sleep.
//! - `BedBlock.useWithoutItem`: foot clicks redirect to the head (a lone foot
//!   consumes silently); occupied → kick the villager or overlay
//!   `block.minecraft.bed.occupied`; then `startSleepInBed`, problems shown
//!   as overlays.
//! - Skip-night (`ServerLevel.tick`): `areEnoughSleeping(pct)` (sleeping ≥
//!   need) **and** `areEnoughDeepSleeping(pct)` (sleep timer ≥ 100 ≥ need,
//!   `need = max(1, ceil(active × pct / 100))`) → advance clock to
//!   `WAKE_UP_FROM_SLEEP` (= 0, `timeline/day.json`), `wakeUpAllPlayers`, and
//!   `resetWeatherCycle` when raining and `ADVANCE_WEATHER` holds.
//! - Gamerule: `players_sleeping_percentage` registers `(PLAYER, 100, 0)` —
//!   default **100**, not 50. P20-05 stores it; until then the constant rules.
//!
//! ## What this slice does and does not do
//!
//! Done: bed-item placement (foot+head, clicker facing), the full attempt
//! chain above with per-reason messages, the 100-tick deep counter, the
//! percentage skip with the morning jump + weather reset, wake on
//! damage/disconnect/bed-break/morning, `occupied` on both halves while
//! slept in, respawn point recorded in memory. Named gaps: no sleep-pose or
//! bed-screen packets (no metadata channel exists — the mechanics run, the
//! client's pose is unverified); no voluntary leave-bed packet (unverified
//! what the real client sends; wake on damage/disconnect/skip/break only);
//! `announceSleepStatus` count message not sent; respawn persistence,
//! `/spawnpoint` and obstructed-respawn redirect are slice 2; bed explosion
//! waits for the Nether (P21); no survival checks on placement (floats where
//! Vanilla would pop); rainfall-only noon sleeping follows the phase scope
//! (night-or-thunder) rather than the jar's exact dimming curve.

use super::Game;
use mc_entity::player::GameMode;
use mc_network::bridge::ConnectionId;

/// Sleep ticks before a sleeper counts as deep (`Player.isSleepingLongEnough`:
/// `sleepCounter >= 100`).
pub(crate) const DEEP_SLEEP_TICKS: u32 = 100;

/// Jar default for `players_sleeping_percentage`, pinned (jar
/// `GameRules.registerInteger(PLAYER, 100, 0)`).
///
/// The skip-night count reads the live stored value
/// (`Game::rules.players_sleeping_percentage`); this constant pins the
/// default the percentage test assumes.
pub(crate) const SLEEPING_PERCENTAGE_DEFAULT: i32 = 100;

/// Reach box half-extents (`isReachableBedBlock`: |dx|≤3, |dy|≤2, |dz|≤3).
const BED_DX: f64 = 3.0;
const BED_DY: f64 = 2.0;
const BED_DZ: f64 = 3.0;

/// Monster box half-extents around the bed's bottom centre (jar: 8.0/5.0).
const MONSTER_DX: f64 = 8.0;
const MONSTER_DY: f64 = 5.0;
const MONSTER_DZ: f64 = 8.0;

/// Morning marker (`timeline/day.json` `wake_up_from_sleep: 0`, measured).
const WAKE_TIME_OF_DAY: i64 = 0;

/// Where a death respawns: exact feet position plus look rotation.
pub(crate) type RespawnDestination = ((f64, f64, f64), (f32, f32));

/// One sleeping session: which bed (head cell) and for how long.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SleepState {
    /// Head cell of the slept-in bed.
    pub bed: (i32, i32, i32),
    /// Ticks spent asleep (deep at [`DEEP_SLEEP_TICKS`]).
    pub ticks: u32,
}

/// Respawn point recorded when a sleep attempt passes the spawn gate
/// (persisted and redirected in slice 2; the single dimension is implicit).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RespawnPoint {
    /// Head cell of the bed, or the commanded point.
    pub pos: (i32, i32, i32),
    /// The sleeper's yaw/pitch at the attempt (jar `RespawnData.of`).
    pub yaw: f32,
    pub pitch: f32,
    /// Jar `RespawnConfig.forced`: beds record false, `/spawnpoint` true. A
    /// forced point spawns blind when its two cells pass; an unforced one
    /// needs its bed.
    pub forced: bool,
}

/// Offset of a horizontal cardinal (`north` = −Z, jar `Direction`).
pub(crate) fn facing_offset(facing: &str) -> (i32, i32, i32) {
    match facing {
        "north" => (0, 0, -1),
        "south" => (0, 0, 1),
        "west" => (-1, 0, 0),
        "east" => (1, 0, 0),
        _ => (0, 0, 0),
    }
}

impl Game {
    /// Resolve the head cell of the bed at `(x, y, z)`, or `None` when the
    /// partner half is missing (a lone foot consumes silently, jar
    /// `useWithoutItem`).
    pub(crate) fn bed_head_of(&self, x: i32, y: i32, z: i32) -> Option<(i32, i32, i32)> {
        let id = self.world.get_block_loaded(x, y, z)?;
        let name = self.registries.blocks.block_name(id).ok()?.to_owned();
        if !name.ends_with("_bed") {
            return None;
        }
        let props = self.registries.blocks.properties_of(id).unwrap_or_default();
        let part = props.iter().find(|(k, _)| k == "part")?.1.clone();
        let facing = props
            .iter()
            .find(|(k, _)| k == "facing")
            .map_or("north", |(_, v)| v.as_str());
        if part == "head" {
            return Some((x, y, z));
        }
        let (dx, dy, dz) = facing_offset(facing);
        let head = (x + dx, y + dy, z + dz);
        let head_id = self.world.get_block_loaded(head.0, head.1, head.2)?;
        let head_name = self.registries.blocks.block_name(head_id).ok()?;
        if head_name != name {
            return None;
        }
        let head_props = self
            .registries
            .blocks
            .properties_of(head_id)
            .unwrap_or_default();
        if head_props.iter().any(|(k, v)| k == "part" && v == "head") {
            Some(head)
        } else {
            None
        }
    }

    /// Whether the player may sleep now: thundering, or dark by the pack's
    /// own sky timeline (`BedRule.WHEN_DARK` = `isDarkOutside`, i.e.
    /// `skyDarken >= 4`, evaluated on our static timeline — rain-only noon
    /// dimming is the named edge this does not model).
    pub(crate) fn can_sleep_now(&self) -> bool {
        if self.weather.thundering {
            return true;
        }
        let time = crate::spawn::time_of_day(self.world_time(), self.time_offset());
        crate::spawn::sky_darken(time) >= 4
    }

    /// Attempt to sleep in the bed at `(x, y, z)` (jar `startSleepInBed`).
    ///
    /// Always handled (every jar path answers `SUCCESS_SERVER`): returns
    /// `true` so the placement path never runs through a bed. Refusals reach
    /// the player as chat lines — the overlay channel does not exist here,
    /// and the one jar-verified key (`block.minecraft.bed.occupied`) is
    /// quoted in the message body rather than as a translation id.
    pub(crate) fn try_sleep(&mut self, id: ConnectionId, x: i32, y: i32, z: i32) -> bool {
        let Some(session) = self.sessions.get(&id) else {
            return true;
        };
        if session.sleeping.is_some() || !session.player.is_alive() {
            return true;
        }
        let Some(head) = self.bed_head_of(x, y, z) else {
            return true;
        };
        // Range: the clicked half or its partner within 3/2/3 of the player.
        let position = session.player.position;
        let in_range = [(x, y, z), head].iter().any(|&(bx, by, bz)| {
            (position.x - (f64::from(bx) + 0.5)).abs() <= BED_DX
                && (position.y - f64::from(by)).abs() <= BED_DY
                && (position.z - (f64::from(bz) + 0.5)).abs() <= BED_DZ
        });
        if !in_range {
            self.send_message(id, "That bed is too far away.");
            return true;
        }
        // Obstruction: both cells above foot and head must be air (jar
        // `bedBlocked` reads `freeAt`; air is this build's honest subset).
        let air = self.registries.blocks.air_id();
        let facing = self
            .bed_facing_of(head)
            .unwrap_or_else(|| "north".to_owned());
        let (fdx, _, fdz) = facing_offset(&facing);
        let foot = (head.0 - fdx, head.1, head.2 - fdz);
        let blocked = [(foot.0, foot.1 + 1, foot.2), (head.0, head.1 + 1, head.2)]
            .iter()
            .any(|&(cx, cy, cz)| {
                self.world
                    .get_block_loaded(cx, cy, cz)
                    .is_none_or(|cell| cell != air)
            });
        if blocked {
            self.send_message(id, "This bed is obstructed.");
            return true;
        }
        // Spawn gate first (jar order): a daytime click still records home.
        // Beds record unforced (a missing bed later spends the point).
        let (yaw, pitch) = (session.player.yaw, session.player.pitch);
        if let Some(session) = self.sessions.get_mut(&id) {
            session.respawn = Some(RespawnPoint {
                pos: head,
                yaw,
                pitch,
                forced: false,
            });
        }
        if !self.can_sleep_now() {
            self.send_message(id, "You can only sleep at night or during thunderstorms.");
            return true;
        }
        // Monsters: skipped in creative (jar `isCreative` gate).
        let creative = self
            .sessions
            .get(&id)
            .is_some_and(|session| session.player.game_mode.is_creative());
        if !creative && self.monsters_near_bed(head) {
            self.send_message(id, "You may not rest now; there are monsters nearby.");
            return true;
        }
        // Occupied: no villagers exist to kick (P23), so any occupant refuses.
        if self.bed_occupied(head) {
            self.send_message(id, "This bed is occupied.");
            return true;
        }
        self.set_bed_occupied(head, true);
        if let Some(session) = self.sessions.get_mut(&id) {
            session.sleeping = Some(SleepState {
                bed: head,
                ticks: 0,
            });
        }
        true
    }

    /// Facing of the bed whose head is at `head` (owned: the assignment it
    /// reads from is a temporary).
    fn bed_facing_of(&self, head: (i32, i32, i32)) -> Option<String> {
        let id = self.world.get_block_loaded(head.0, head.1, head.2)?;
        let props = self.registries.blocks.properties_of(id).ok()?;
        props
            .iter()
            .find(|(k, _)| k == "facing")
            .map(|(_, v)| v.clone())
            .filter(|facing| matches!(facing.as_str(), "north" | "south" | "west" | "east"))
    }

    /// `(name, part, facing)` of a bed state id, all owned (same reason).
    pub(crate) fn bed_part_of(&self, id: i32) -> Option<(String, String, String)> {
        let name = self.registries.blocks.block_name(id).ok()?.to_owned();
        if !name.ends_with("_bed") {
            return None;
        }
        let props = self.registries.blocks.properties_of(id).ok()?;
        let part = props
            .iter()
            .find(|(k, _)| k == "part")
            .map(|(_, v)| v.clone())?;
        let facing = props
            .iter()
            .find(|(k, _)| k == "facing")
            .map(|(_, v)| v.clone())?;
        Some((name, part, facing))
    }

    /// Whether either half of the bed reads `occupied`.
    fn bed_occupied(&self, head: (i32, i32, i32)) -> bool {
        let facing = self
            .bed_facing_of(head)
            .unwrap_or_else(|| "north".to_owned());
        let (dx, _, dz) = facing_offset(&facing);
        [(head.0 - dx, head.1, head.2 - dz), head]
            .iter()
            .any(|&(x, y, z)| {
                self.world
                    .get_block_loaded(x, y, z)
                    .and_then(|id| self.registries.blocks.properties_of(id).ok())
                    .and_then(|props| {
                        props
                            .iter()
                            .find(|(k, _)| k == "occupied")
                            .map(|(_, v)| v == "true")
                    })
                    .unwrap_or(false)
            })
    }

    /// Write `occupied` on both halves (best-effort: an unresolvable state
    /// warns and the sleep still stands — the flag is shared knowledge, not
    /// the sleep itself).
    pub(crate) fn set_bed_occupied(&mut self, head: (i32, i32, i32), occupied: bool) {
        let facing = self
            .bed_facing_of(head)
            .unwrap_or_else(|| "north".to_owned());
        let (dx, _, dz) = facing_offset(&facing);
        for (x, y, z) in [(head.0 - dx, head.1, head.2 - dz), head] {
            let Some(id) = self.world.get_block_loaded(x, y, z) else {
                continue;
            };
            let Ok(name) = self.registries.blocks.block_name(id) else {
                continue;
            };
            let name = name.to_owned();
            if !name.ends_with("_bed") {
                continue;
            }
            let mut props = self.registries.blocks.properties_of(id).unwrap_or_default();
            let Some(entry) = props.iter_mut().find(|(k, _)| k == "occupied") else {
                continue;
            };
            entry.1 = occupied.to_string();
            let Ok(new_id) = self.registries.blocks.state_id(&name, &props) else {
                continue;
            };
            if self.world.set_block(x, y, z, new_id).is_ok() {
                self.block_feed(x, y, z, new_id);
            }
        }
    }

    /// Any living hostile mob in the 16×10×16 box around the bed's bottom
    /// centre (jar `tick` gate: `Monster` class, ±8/±5).
    fn monsters_near_bed(&self, head: (i32, i32, i32)) -> bool {
        let (cx, cy, cz) = (
            f64::from(head.0) + 0.5,
            f64::from(head.1),
            f64::from(head.2) + 0.5,
        );
        self.entities.ids().any(|entity_id| {
            let Some(entity) = self.entities.get(entity_id) else {
                return false;
            };
            if entity.removed || entity.health <= 0.0 {
                return false;
            }
            let mc_entity::EntityBody::Mob(mob) = &entity.body else {
                return false;
            };
            if !mob.kind.is_hostile() {
                return false;
            }
            (entity.position.x - cx).abs() <= MONSTER_DX
                && (entity.position.y - cy).abs() <= MONSTER_DY
                && (entity.position.z - cz).abs() <= MONSTER_DZ
        })
    }

    /// Wake one player: clear the bed's `occupied` and forget the sleep.
    pub(crate) fn wake(&mut self, id: ConnectionId) {
        let Some(bed) = self
            .sessions
            .get(&id)
            .and_then(|session| session.sleeping.map(|s| s.bed))
        else {
            return;
        };
        self.set_bed_occupied(bed, false);
        if let Some(session) = self.sessions.get_mut(&id) {
            session.sleeping = None;
        }
    }

    /// Wake every session sleeping in either half of the bed at `(x, y, z)`
    /// (bed-break path: the halves are resolved, not assumed).
    pub(crate) fn wake_sleepers_on_bed(&mut self, x: i32, y: i32, z: i32) {
        let sleepers: Vec<ConnectionId> = self
            .sessions
            .iter()
            .filter_map(|(id, session)| {
                session.sleeping.and_then(|sleep| {
                    let facing = self
                        .bed_facing_of(sleep.bed)
                        .unwrap_or_else(|| "north".to_owned());
                    let (dx, _, dz) = facing_offset(&facing);
                    let foot = (sleep.bed.0 - dx, sleep.bed.1, sleep.bed.2 - dz);
                    if sleep.bed == (x, y, z) || foot == (x, y, z) {
                        Some(*id)
                    } else {
                        None
                    }
                })
            })
            .collect();
        for id in sleepers {
            self.wake(id);
        }
    }

    /// Where a death respawns: the recorded point when it still validates,
    /// else world spawn with the message (P20-04 slice 2).
    ///
    /// Jar `findRespawnAndUseSpawnBlock`: a bed head with `canSetSpawn`
    /// (always here) resolves through the stand-up search — this build's
    /// honest subset is the two air cells above foot and head, spawning
    /// directly above the head. A commanded point (`forced`, which
    /// `/spawnpoint` always sets, jar `SetSpawnCommand`) spawns at
    /// `(x+0.5, y+0.1, z+0.5)` when both cells pass (`isPossibleToRespawnIn-
    /// This`, subset to air — water/snow above a forced point is a named
    /// edge). Anything else is spent — cleared with one message, so the next
    /// death goes to world spawn silently, exactly like the jar consuming a
    /// broken `RespawnConfig`.
    pub(crate) fn respawn_destination(&mut self, id: ConnectionId) -> Option<RespawnDestination> {
        let point = self.sessions.get(&id)?.respawn?;
        // Judge the point as it stands: a death far from home (or a command
        // point nobody has visited) must load its chunk first, or every
        // unloaded point reads as missing.
        self.load_chunk(mc_world::ChunkPos::new(point.pos.0 >> 4, point.pos.2 >> 4));
        let spend = |game: &mut Self| {
            if let Some(session) = game.sessions.get_mut(&id) {
                session.respawn = None;
            }
            game.send_message(id, "Your home bed was missing or obstructed.");
        };
        if !point.forced {
            // Bed points: the head must still be a bed with headroom.
            // Standing room is directly above the head — the two air cells
            // the check just confirmed.
            let (hx, hy, hz) = point.pos;
            let valid = self
                .bed_head_of(hx, hy, hz)
                .is_some_and(|head| head == point.pos)
                && {
                    let facing = self
                        .bed_facing_of(point.pos)
                        .unwrap_or_else(|| "north".to_owned());
                    let (dx, _, dz) = facing_offset(&facing);
                    let air = self.registries.blocks.air_id();
                    [(hx - dx, hy + 1, hz - dz), (hx, hy + 1, hz)]
                        .iter()
                        .all(|(x, y, z)| {
                            self.world
                                .get_block_loaded(*x, *y, *z)
                                .is_some_and(|cell| cell == air)
                        })
                };
            if !valid {
                spend(self);
                return None;
            }
            return Some((
                (
                    f64::from(hx) + 0.5,
                    f64::from(hy) + 1.0,
                    f64::from(hz) + 0.5,
                ),
                (point.yaw, point.pitch),
            ));
        }
        // Forced points: spawn blind when both cells pass (jar `+0.1` nudge
        // against ground clip).
        let (px, py, pz) = point.pos;
        let air = self.registries.blocks.air_id();
        let passable = [(px, py, pz), (px, py + 1, pz)].iter().all(|(x, y, z)| {
            self.world
                .get_block_loaded(*x, *y, *z)
                .is_some_and(|cell| cell == air)
        });
        if !passable {
            spend(self);
            return None;
        }
        Some((
            (
                f64::from(px) + 0.5,
                f64::from(py) + 0.1,
                f64::from(pz) + 0.5,
            ),
            (point.yaw, point.pitch),
        ))
    }

    /// Advance sleep timers and skip the night when enough sleep deeply.
    ///
    /// Runs at the end of the Players phase (positions final, like the jar's
    /// per-tick evaluation): `need = max(1, ceil(active × pct / 100))` with
    /// the jar-default 100 until P20-05 stores the rule; deep means ≥ 100
    /// ticks (`isSleepingLongEnough`). The skip jumps `time_offset` to
    /// morning (`WAKE_UP_FROM_SLEEP` = 0, measured in `day.json`), resets the
    /// weather when raining, and wakes everyone — all silent, as the jar's
    /// path is (clients learn from the clock entries and easing levels).
    pub(crate) fn tick_sleep(&mut self) {
        for session in self.sessions.values_mut() {
            if let Some(sleep) = session.sleeping.as_mut() {
                sleep.ticks = sleep.ticks.saturating_add(1);
            }
        }
        let active = self
            .sessions
            .values()
            .filter(|session| session.player.game_mode != GameMode::Spectator)
            .count();
        if active == 0 {
            return;
        }
        let sleeping = self
            .sessions
            .values()
            .filter(|session| session.sleeping.is_some())
            .count();
        let deep = self
            .sessions
            .values()
            .filter(|session| {
                session
                    .sleeping
                    .is_some_and(|sleep| sleep.ticks >= DEEP_SLEEP_TICKS)
            })
            .count();
        let need = (active
            .saturating_mul(self.rules.players_sleeping_percentage.max(0) as usize)
            .saturating_add(99)
            / 100)
            .max(1);
        if sleeping < need || deep < need {
            return;
        }
        let time = crate::spawn::time_of_day(self.world_time(), self.time_offset());
        self.set_time_offset(self.time_offset() + (WAKE_TIME_OF_DAY - time).rem_euclid(24_000));
        if self.weather.raining {
            // Silent like `resetWeatherCycle`: no START/STOP event — the
            // easing levels carry the change, as they do every tick.
            self.weather.raining = false;
            self.weather.thundering = false;
            self.weather.rain_time = 0;
            self.weather.thunder_time = 0;
        }
        let sleepers: Vec<ConnectionId> = self
            .sessions
            .keys()
            .filter(|id| {
                self.sessions
                    .get(id)
                    .is_some_and(|session| session.sleeping.is_some())
            })
            .copied()
            .collect();
        for id in sleepers {
            self.wake(id);
        }
    }
}
