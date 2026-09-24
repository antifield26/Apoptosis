//! Who is running a command, and what they are allowed to do (P07-04).
//!
//! ## Why the source is a trait-free struct
//!
//! Vanilla's `CommandSourceStack` carries the level, the position, the entity, the
//! rotation, the dimension and the server, and every command reaches into it for
//! whatever it needs. Reproducing that shape here would mean `mc-command` depending on
//! the world, the entity model and the server — the crate would stop being testable in
//! isolation, which is the property that makes the dispatcher's hostile-input tests
//! cheap.
//!
//! So a [`CommandSource`] is the **smallest description** a permission check and a
//! position-relative argument need: who, where, with what authority. Commands that need
//! more (the world, the player list) take it from their execution context in
//! `mc-server`, which is P07-07's job.
//!
//! ## Permission levels
//!
//! Vanilla's four levels, with the same meanings:
//!
//! | Level | Who |
//! |---|---|
//! | 0 | everyone |
//! | 1 | anyone who can bypass spawn protection |
//! | 2 | command blocks and operators |
//! | 3 | multiplayer administrators |
//! | 4 | the server console |
//!
//! **The names are Vanilla's; the mapping from a player to a level is this project's**,
//! read from `ops.json` (P14-02 / KD-33) — an offline-mode player is level 0 unless the
//! operator grants otherwise, and the console is level 4.

/// What a source is allowed to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PermissionLevel {
    /// Everyone, including a player who has never been granted anything.
    All,
    /// Bypasses spawn protection.
    BypassSpawnProtection,
    /// Command blocks, and operators.
    Operator,
    /// Multiplayer administrators.
    Administrator,
    /// The server console.
    Console,
}

impl PermissionLevel {
    /// The numeric level Vanilla uses.
    #[must_use]
    pub const fn level(self) -> u8 {
        match self {
            Self::All => 0,
            Self::BypassSpawnProtection => 1,
            Self::Operator => 2,
            Self::Administrator => 3,
            Self::Console => 4,
        }
    }

    /// Recognise a numeric level.
    #[must_use]
    pub const fn from_level(level: u8) -> Option<Self> {
        match level {
            0 => Some(Self::All),
            1 => Some(Self::BypassSpawnProtection),
            2 => Some(Self::Operator),
            3 => Some(Self::Administrator),
            4 => Some(Self::Console),
            _ => None,
        }
    }

    /// Stable name for logs.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::BypassSpawnProtection => "bypass_spawn_protection",
            Self::Operator => "operator",
            Self::Administrator => "administrator",
            Self::Console => "console",
        }
    }
}

impl std::fmt::Display for PermissionLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}({})", self.name(), self.level())
    }
}

/// What kind of thing issued a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// A connected player.
    Player,
    /// The server console.
    Console,
    /// A command block, once those exist.
    CommandBlock,
}

impl SourceKind {
    /// Stable name for logs.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Player => "player",
            Self::Console => "console",
            Self::CommandBlock => "command_block",
        }
    }
}

/// A position a command can be resolved relative to.
///
/// `f64` because a player's position is fractional and `~` resolves against it; the
/// block coordinate a `BlockPos` argument produces is floored at resolution time, by the
/// command that needs it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SourcePosition {
    /// X.
    pub x: f64,
    /// Y.
    pub y: f64,
    /// Z.
    pub z: f64,
}

impl SourcePosition {
    /// The origin, which is where the console sits.
    #[must_use]
    pub const fn origin() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        }
    }

    /// A position.
    #[must_use]
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    /// Whether every component is finite.
    #[must_use]
    pub fn is_finite(&self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }
}

impl Default for SourcePosition {
    fn default() -> Self {
        Self::origin()
    }
}

/// Who is running a command.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandSource {
    /// What kind of source it is.
    pub kind: SourceKind,
    /// Its name, for messages: a player's username, or `"Server"` for the console.
    pub name: String,
    /// What it may do.
    pub permission: PermissionLevel,
    /// Where it is, for `~` and for a distance-limited command.
    pub position: SourcePosition,
    /// Which dimension it is in, as a namespaced id.
    pub dimension: String,
    /// Yaw in degrees (`execute rotated` / `facing` write this).
    pub yaw: f32,
    /// Pitch in degrees.
    pub pitch: f32,
    /// Whether `facing`/local coordinates measure from the eyes or the feet.
    pub anchor: crate::execute::Anchor,
}

impl CommandSource {
    /// The server console: level 4, at the origin, in the overworld.
    #[must_use]
    pub fn console() -> Self {
        Self {
            kind: SourceKind::Console,
            name: "Server".to_owned(),
            permission: PermissionLevel::Console,
            position: SourcePosition::origin(),
            dimension: "minecraft:overworld".to_owned(),
            yaw: 0.0,
            pitch: 0.0,
            anchor: crate::execute::Anchor::default(),
        }
    }

    /// A player at a position, with an explicit permission level.
    #[must_use]
    pub fn player(name: impl Into<String>, position: SourcePosition) -> Self {
        Self {
            kind: SourceKind::Player,
            name: name.into(),
            // A fresh player has no grants; the operator's permission file is not read
            // yet, so anything above `All` must be granted explicitly (P07-04).
            permission: PermissionLevel::All,
            position,
            dimension: "minecraft:overworld".to_owned(),
            yaw: 0.0,
            pitch: 0.0,
            anchor: crate::execute::Anchor::default(),
        }
    }

    /// The same source with a permission level.
    #[must_use]
    pub fn with_permission(mut self, permission: PermissionLevel) -> Self {
        self.permission = permission;
        self
    }

    /// The same source with a rotation.
    #[must_use]
    pub const fn with_rotation(mut self, yaw: f32, pitch: f32) -> Self {
        self.yaw = yaw;
        self.pitch = pitch;
        self
    }

    /// Whether this source may run a command requiring `required`.
    #[must_use]
    pub fn may_use(&self, required: PermissionLevel) -> bool {
        self.permission >= required
    }

    /// Whether this source is a player.
    #[must_use]
    pub const fn is_player(&self) -> bool {
        matches!(self.kind, SourceKind::Player)
    }

    /// The block position `~` resolves against: the source's position, floored.
    ///
    /// Returns `None` for a non-finite position, so a hostile position cannot produce a
    /// nonsensical absolute coordinate.
    #[must_use]
    pub fn block_position(&self) -> Option<(i32, i32, i32)> {
        if !self.position.is_finite() {
            return None;
        }
        // A world coordinate is far inside `i32`, and the finiteness check above is what
        // makes the narrowing safe: a non-finite or absurd value returns `None` instead
        // of saturating to a corner of the world.
        let clamp = |value: f64| {
            let floored = value.floor();
            if floored < f64::from(i32::MIN) {
                i32::MIN
            } else if floored > f64::from(i32::MAX) {
                i32::MAX
            } else {
                // The bounds are checked immediately above, so this cannot truncate.
                #[allow(clippy::cast_possible_truncation)]
                {
                    floored as i32
                }
            }
        };
        Some((
            clamp(self.position.x),
            clamp(self.position.y),
            clamp(self.position.z),
        ))
    }
}

#[cfg(test)]
// Coordinates are produced from exact literals and compared with them, so these are
// exact comparisons rather than approximate ones.
#[allow(clippy::float_cmp)]
mod tests {
    use super::{CommandSource, PermissionLevel, SourceKind, SourcePosition};

    #[test]
    fn permission_levels_round_trip_and_order() {
        for level in 0..=4u8 {
            let parsed = PermissionLevel::from_level(level).expect("a known level");
            assert_eq!(parsed.level(), level);
            assert!(!parsed.name().is_empty());
            assert!(parsed.to_string().contains(&level.to_string()));
        }
        assert_eq!(PermissionLevel::from_level(5), None);
        assert_eq!(PermissionLevel::from_level(255), None);

        // Ordering is what `may_use` relies on.
        assert!(PermissionLevel::All < PermissionLevel::Operator);
        assert!(PermissionLevel::Operator < PermissionLevel::Console);
    }

    #[test]
    fn a_source_may_use_at_or_below_its_level() {
        let player = CommandSource::player("Alex", SourcePosition::origin());
        assert!(player.may_use(PermissionLevel::All));
        assert!(!player.may_use(PermissionLevel::Operator));
        assert!(!player.may_use(PermissionLevel::Console));

        let op = player.clone().with_permission(PermissionLevel::Operator);
        assert!(op.may_use(PermissionLevel::All));
        assert!(op.may_use(PermissionLevel::Operator));
        assert!(!op.may_use(PermissionLevel::Administrator));

        let console = CommandSource::console();
        for level in [
            PermissionLevel::All,
            PermissionLevel::BypassSpawnProtection,
            PermissionLevel::Operator,
            PermissionLevel::Administrator,
            PermissionLevel::Console,
        ] {
            assert!(console.may_use(level), "the console may use {level}");
        }
    }

    #[test]
    fn the_console_is_a_console_and_a_player_is_a_player() {
        let console = CommandSource::console();
        assert_eq!(console.kind, SourceKind::Console);
        assert!(!console.is_player());
        assert_eq!(console.name, "Server");

        let player = CommandSource::player("Alex", SourcePosition::new(1.5, 64.0, -2.5));
        assert!(player.is_player());
        assert_eq!(player.kind, SourceKind::Player);
        assert_eq!(
            player.permission,
            PermissionLevel::All,
            "no grants by default"
        );
    }

    #[test]
    fn source_kinds_have_names() {
        for kind in [
            SourceKind::Player,
            SourceKind::Console,
            SourceKind::CommandBlock,
        ] {
            assert!(!kind.name().is_empty());
        }
    }

    #[test]
    fn tilde_resolves_against_the_floored_source_position() {
        let player = CommandSource::player("Alex", SourcePosition::new(1.9, 64.0, -2.1));
        assert_eq!(player.block_position(), Some((1, 64, -3)));

        // A non-finite position yields nothing rather than a nonsensical coordinate.
        let broken = CommandSource::player("Alex", SourcePosition::new(f64::NAN, 0.0, 0.0));
        assert_eq!(broken.block_position(), None);
        assert!(!broken.position.is_finite());
    }

    #[test]
    fn position_helpers() {
        assert!(SourcePosition::origin().is_finite());
        assert!(SourcePosition::default().is_finite());
        assert!(SourcePosition::new(1.0, 2.0, 3.0).is_finite());
        assert!(!SourcePosition::new(f64::INFINITY, 0.0, 0.0).is_finite());
        assert_eq!(SourcePosition::new(1.0, 2.0, 3.0).x, 1.0);
    }
}
