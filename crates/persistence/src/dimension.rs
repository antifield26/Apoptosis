//! Dimensions and on-disk layout (P03-09).
//!
//! The 26.1 world layout moved dimensions out of the world root
//! (`docs/research/protocol-baseline.md` section 3, confirmed by listing a real
//! generated world):
//!
//! ```text
//! <world>/level.dat
//! <world>/data/minecraft/*.dat                     (global data)
//! <world>/dimensions/<namespace>/<value>/region/r.X.Z.mca
//! <world>/dimensions/<namespace>/<value>/entities/r.X.Z.mca
//! <world>/dimensions/<namespace>/<value>/poi/r.X.Z.mca
//! <world>/dimensions/<namespace>/<value>/data/minecraft/*.dat
//! ```
//!
//! Pre-26.1 worlds used `<world>/region` for the overworld and `<world>/DIM-1`,
//! `<world>/DIM1` for the nether and the end. The reader supports both (new
//! layout first, legacy fallback with a deprecation warning); the writer only
//! ever creates the new layout, as decided in ADR-0001 D-04.
//!
//! [`Dimension`] keys go through [`ResourceId`], whose validation already
//! rejects `.`/`..` path segments, so a dimension key can never escape the world
//! directory (AGENTS.md section 10).

use mc_core::error::{ServerError, ServerResult};
use mc_core::ids::ResourceId;
use std::fmt;
use std::path::{Path, PathBuf};

/// Resource key of a world dimension.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Dimension {
    /// `minecraft:overworld` — the default dimension.
    Overworld,
    /// `minecraft:the_nether`.
    Nether,
    /// `minecraft:the_end`.
    End,
    /// Any other dimension key (custom datapack dimensions).
    Custom(ResourceId),
}

/// Vanilla dimension keys, as they appear in `level.dat` and registry payloads.
pub const OVERWORLD_KEY: &str = "minecraft:overworld";
/// `minecraft:the_nether`.
pub const NETHER_KEY: &str = "minecraft:the_nether";
/// `minecraft:the_end`.
pub const THE_END_KEY: &str = "minecraft:the_end";

impl Dimension {
    /// Parse a dimension key, recognising the three Vanilla dimensions.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] when the key is not a valid resource id.
    pub fn parse(key: &str) -> ServerResult<Self> {
        match key {
            OVERWORLD_KEY => Ok(Self::Overworld),
            NETHER_KEY => Ok(Self::Nether),
            THE_END_KEY => Ok(Self::End),
            other => Ok(Self::Custom(ResourceId::parse(other)?)),
        }
    }

    /// The dimension's resource key.
    #[must_use]
    pub fn key(&self) -> String {
        match self {
            Self::Overworld => OVERWORLD_KEY.to_owned(),
            Self::Nether => NETHER_KEY.to_owned(),
            Self::End => THE_END_KEY.to_owned(),
            Self::Custom(id) => id.to_string(),
        }
    }

    /// Legacy (pre-26.1) directory holding this dimension's region files,
    /// relative to the world root. `None` for dimensions that never had one.
    #[must_use]
    pub fn legacy_dir(&self) -> Option<&'static str> {
        match self {
            Self::Overworld => Some(""),
            Self::Nether => Some("DIM-1"),
            Self::End => Some("DIM1"),
            Self::Custom(_) => None,
        }
    }

    /// Whether this is one of the three Vanilla dimensions.
    #[must_use]
    pub const fn is_vanilla(&self) -> bool {
        !matches!(self, Self::Custom(_))
    }
}

impl fmt::Display for Dimension {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.key())
    }
}

/// File-name prefix used for region files (`r.<x>.<z>.mca`).
pub const REGION_PREFIX: &str = "r.";
/// Region file extension.
pub const REGION_EXTENSION: &str = ".mca";

/// `r.<x>.<z>.mca` for a region coordinate pair.
#[must_use]
pub fn region_file_name(region_x: i32, region_z: i32) -> String {
    format!("{REGION_PREFIX}{region_x}.{region_z}{REGION_EXTENSION}")
}

/// The per-dimension storage folders, all resolved against the world root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DimensionLayout {
    root: PathBuf,
    legacy: bool,
}

impl DimensionLayout {
    /// Layout for the modern `<world>/dimensions/<ns>/<value>` tree.
    #[must_use]
    pub fn modern(world_root: &Path, dimension: &Dimension) -> Self {
        let key = dimension.key();
        let (namespace, value) = key.split_once(':').unwrap_or(("minecraft", &key));
        Self {
            root: world_root.join("dimensions").join(namespace).join(value),
            legacy: false,
        }
    }

    /// Layout for a pre-26.1 world (`<world>`, `<world>/DIM-1`, `<world>/DIM1`).
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when the dimension has no legacy layout
    /// (custom dimensions only ever existed in the modern tree).
    pub fn legacy(world_root: &Path, dimension: &Dimension) -> ServerResult<Self> {
        let dir = dimension.legacy_dir().ok_or_else(|| {
            ServerError::Operational(format!(
                "dimension {dimension} has no pre-26.1 layout to fall back to"
            ))
        })?;
        Ok(Self {
            root: if dir.is_empty() {
                world_root.to_path_buf()
            } else {
                world_root.join(dir)
            },
            legacy: true,
        })
    }

    /// Folder holding `r.X.Z.mca` chunk files.
    #[must_use]
    pub fn region_dir(&self) -> PathBuf {
        self.root.join("region")
    }

    /// Folder holding entity region files.
    #[must_use]
    pub fn entities_dir(&self) -> PathBuf {
        self.root.join("entities")
    }

    /// Folder holding point-of-interest region files.
    #[must_use]
    pub fn poi_dir(&self) -> PathBuf {
        self.root.join("poi")
    }

    /// Folder holding per-dimension `data/minecraft/*.dat` files.
    #[must_use]
    pub fn data_dir(&self) -> PathBuf {
        self.root.join("data").join("minecraft")
    }

    /// Path of one region file.
    #[must_use]
    pub fn region_path(&self, region_x: i32, region_z: i32) -> PathBuf {
        self.region_dir().join(region_file_name(region_x, region_z))
    }

    /// Whether this is a legacy (pre-26.1) layout.
    #[must_use]
    pub const fn is_legacy(&self) -> bool {
        self.legacy
    }

    /// Root folder of this dimension's tree.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::{Dimension, DimensionLayout, OVERWORLD_KEY, region_file_name};
    use mc_core::error::ServerError;
    use std::path::Path;

    #[test]
    fn vanilla_keys_round_trip() {
        for key in [
            "minecraft:overworld",
            "minecraft:the_nether",
            "minecraft:the_end",
        ] {
            let dimension = Dimension::parse(key).expect("valid");
            assert_eq!(dimension.key(), key);
            assert!(dimension.is_vanilla());
            assert_eq!(dimension.to_string(), key);
        }
        assert_eq!(
            Dimension::parse(OVERWORLD_KEY).expect("valid"),
            Dimension::Overworld
        );
    }

    #[test]
    fn custom_dimensions_are_validated_resource_ids() {
        let custom = Dimension::parse("mypack:skylands").expect("valid");
        assert!(!custom.is_vanilla());
        assert_eq!(custom.key(), "mypack:skylands");
        // A bare value defaults to the minecraft namespace, like every id.
        assert_eq!(
            Dimension::parse("skylands").expect("valid").key(),
            "minecraft:skylands"
        );
    }

    #[test]
    fn hostile_dimension_keys_are_rejected() {
        for bad in [
            "",
            "..",
            "minecraft:..",
            "a/../../etc",
            "minecraft:..",
            "NS:value",
        ] {
            assert!(
                Dimension::parse(bad).is_err(),
                "should reject dimension key {bad:?}"
            );
        }
        // Belt and braces: a traversal payload never reaches a path.
        let err = Dimension::parse("minecraft:..").expect_err("rejected");
        assert!(matches!(err, ServerError::Protocol(_)), "{err:?}");
    }

    #[test]
    fn modern_layout_matches_the_measured_26_1_tree() {
        let layout = DimensionLayout::modern(Path::new("/srv/world"), &Dimension::Overworld);
        let region = layout.region_path(-2, -1);
        let text = region.to_string_lossy().replace('\\', "/");
        assert_eq!(
            text,
            "/srv/world/dimensions/minecraft/overworld/region/r.-2.-1.mca"
        );
        assert!(!layout.is_legacy());
        assert!(layout.entities_dir().ends_with("overworld/entities"));
        assert!(layout.data_dir().ends_with("overworld/data/minecraft"));
    }

    #[test]
    fn custom_dimension_layout_uses_the_namespace_folder() {
        let dimension = Dimension::parse("mypack:skylands").expect("valid");
        let layout = DimensionLayout::modern(Path::new("world"), &dimension);
        let text = layout.region_dir().to_string_lossy().replace('\\', "/");
        assert_eq!(text, "world/dimensions/mypack/skylands/region");
        assert!(DimensionLayout::legacy(Path::new("world"), &dimension).is_err());
    }

    #[test]
    fn legacy_layouts_map_to_the_documented_folders() {
        let root = Path::new("world");
        let overworld =
            DimensionLayout::legacy(root, &Dimension::Overworld).expect("overworld legacy");
        assert_eq!(overworld.region_dir(), root.join("region"));
        let nether = DimensionLayout::legacy(root, &Dimension::Nether).expect("nether legacy");
        assert_eq!(nether.region_dir(), root.join("DIM-1").join("region"));
        let end = DimensionLayout::legacy(root, &Dimension::End).expect("end legacy");
        assert_eq!(end.region_dir(), root.join("DIM1").join("region"));
        assert!(overworld.is_legacy() && nether.is_legacy() && end.is_legacy());
    }

    #[test]
    fn region_file_names_use_vanilla_convention() {
        assert_eq!(region_file_name(-2, -1), "r.-2.-1.mca");
        assert_eq!(region_file_name(0, 33), "r.0.33.mca");
    }
}
