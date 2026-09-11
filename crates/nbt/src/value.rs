//! The tag model plus typed accessors (P03-01).
//!
//! Compound entries keep insertion order in a `Vec` instead of a hash map:
//! round-tripping a Vanilla file then preserves its original key order, which
//! makes byte-level golden comparisons possible and keeps writes reproducible
//! (AGENTS.md section 3.6). Lookups follow Vanilla's `CompoundTag` semantics —
//! the **last** entry with a given name wins — so a duplicate key never changes
//! the decoded meaning while still surviving a round trip.

/// Binary tag ids, matching `net.minecraft.nbt.TagTypes`.
pub mod tag {
    /// End marker; also the element type of a typed empty list.
    pub const END: u8 = 0;
    /// 1-byte signed integer.
    pub const BYTE: u8 = 1;
    /// 2-byte signed integer.
    pub const SHORT: u8 = 2;
    /// 4-byte signed integer.
    pub const INT: u8 = 3;
    /// 8-byte signed integer.
    pub const LONG: u8 = 4;
    /// 4-byte float.
    pub const FLOAT: u8 = 5;
    /// 8-byte float.
    pub const DOUBLE: u8 = 6;
    /// Byte array.
    pub const BYTE_ARRAY: u8 = 7;
    /// Modified UTF-8 string.
    pub const STRING: u8 = 8;
    /// Heterogeneous-except-type list.
    pub const LIST: u8 = 9;
    /// Named tag container.
    pub const COMPOUND: u8 = 10;
    /// Int array.
    pub const INT_ARRAY: u8 = 11;
    /// Long array.
    pub const LONG_ARRAY: u8 = 12;

    /// Human-readable tag name, for diagnostics and error messages.
    #[must_use]
    pub const fn name(id: u8) -> &'static str {
        match id {
            END => "TAG_End",
            BYTE => "TAG_Byte",
            SHORT => "TAG_Short",
            INT => "TAG_Int",
            LONG => "TAG_Long",
            FLOAT => "TAG_Float",
            DOUBLE => "TAG_Double",
            BYTE_ARRAY => "TAG_Byte_Array",
            STRING => "TAG_String",
            LIST => "TAG_List",
            COMPOUND => "TAG_Compound",
            INT_ARRAY => "TAG_Int_Array",
            LONG_ARRAY => "TAG_Long_Array",
            _ => "TAG_Unknown",
        }
    }
}

/// An NBT value tree.
#[derive(Debug, Clone, PartialEq)]
pub enum NbtTag {
    /// 1-byte signed integer.
    Byte(i8),
    /// 2-byte signed integer.
    Short(i16),
    /// 4-byte signed integer.
    Int(i32),
    /// 8-byte signed integer. (Named `Int`/`Long` as in Vanilla.)
    Long(i64),
    /// 4-byte float.
    Float(f32),
    /// 8-byte float.
    Double(f64),
    /// Byte array.
    ByteArray(Vec<u8>),
    /// String (modified UTF-8 on the wire).
    String(String),
    /// List of same-typed tags.
    List(Vec<NbtTag>),
    /// Ordered name/value pairs.
    Compound(Vec<(String, NbtTag)>),
    /// Int array.
    IntArray(Vec<i32>),
    /// Long array.
    LongArray(Vec<i64>),
}

impl NbtTag {
    /// Tag id for this value.
    #[must_use]
    pub fn tag_id(&self) -> u8 {
        match self {
            Self::Byte(_) => tag::BYTE,
            Self::Short(_) => tag::SHORT,
            Self::Int(_) => tag::INT,
            Self::Long(_) => tag::LONG,
            Self::Float(_) => tag::FLOAT,
            Self::Double(_) => tag::DOUBLE,
            Self::ByteArray(_) => tag::BYTE_ARRAY,
            Self::String(_) => tag::STRING,
            Self::List(_) => tag::LIST,
            Self::Compound(_) => tag::COMPOUND,
            Self::IntArray(_) => tag::INT_ARRAY,
            Self::LongArray(_) => tag::LONG_ARRAY,
        }
    }

    /// Name of this value's tag type for diagnostics.
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        tag::name(self.tag_id())
    }

    /// Build a compound from name/value pairs.
    #[must_use]
    pub fn compound(entries: impl IntoIterator<Item = (String, NbtTag)>) -> Self {
        Self::Compound(entries.into_iter().collect())
    }

    /// Borrow the entries of a compound, or `None` for any other tag.
    #[must_use]
    pub fn entries(&self) -> Option<&[(String, NbtTag)]> {
        match self {
            Self::Compound(entries) => Some(entries),
            _ => None,
        }
    }

    /// Look up a compound entry. Later duplicates win (Vanilla `put` semantics).
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&NbtTag> {
        self.entries()?
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value)
    }

    /// Whether a compound contains `name`.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// Insert or replace an entry, keeping Vanilla's last-wins ordering.
    ///
    /// The first occurrence is overwritten in place when present so the key
    /// order stays stable across load/save cycles; otherwise the entry is
    /// appended. Any further duplicates are removed, so the result has exactly
    /// one entry for `name`.
    pub fn insert(&mut self, name: &str, value: NbtTag) {
        let Self::Compound(entries) = self else {
            return;
        };
        match entries.iter().position(|(key, _)| key == name) {
            Some(index) => {
                entries[index].1 = value;
                let mut seen = false;
                entries.retain(|(key, _)| {
                    if key == name {
                        if seen {
                            return false;
                        }
                        seen = true;
                    }
                    true
                });
            }
            None => entries.push((name.to_owned(), value)),
        }
    }

    /// Remove an entry; returns whether anything was removed.
    pub fn remove(&mut self, name: &str) -> bool {
        let Self::Compound(entries) = self else {
            return false;
        };
        let before = entries.len();
        entries.retain(|(key, _)| key != name);
        entries.len() != before
    }

    /// Numeric view of any integer tag, for tolerant reads.
    ///
    /// Vanilla's codecs are type-strict, but hand-edited or tool-written worlds
    /// routinely store an `Int` where the schema says `Long` (or a `Byte` where
    /// it says `Int`). Accepting any integer type that fits keeps those worlds
    /// loadable without weakening validation of the actual value.
    #[must_use]
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Byte(v) => Some(i64::from(*v)),
            Self::Short(v) => Some(i64::from(*v)),
            Self::Int(v) => Some(i64::from(*v)),
            Self::Long(v) => Some(*v),
            _ => None,
        }
    }

    /// Numeric view of any float tag (`Float`/`Double`).
    #[must_use]
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Float(v) => Some(f64::from(*v)),
            Self::Double(v) => Some(*v),
            _ => None,
        }
    }

    /// Integer value of a compound entry, if the entry exists and is an integer.
    #[must_use]
    pub fn get_i64(&self, name: &str) -> Option<i64> {
        self.get(name)?.as_i64()
    }

    /// `i32` value of a compound entry, rejecting out-of-range integers.
    #[must_use]
    pub fn get_i32(&self, name: &str) -> Option<i32> {
        i32::try_from(self.get_i64(name)?).ok()
    }

    /// `i8` value of a compound entry, rejecting out-of-range integers.
    #[must_use]
    pub fn get_i8(&self, name: &str) -> Option<i8> {
        i8::try_from(self.get_i64(name)?).ok()
    }

    /// `i16` value of a compound entry, rejecting out-of-range integers.
    #[must_use]
    pub fn get_i16(&self, name: &str) -> Option<i16> {
        i16::try_from(self.get_i64(name)?).ok()
    }

    /// Boolean value of a compound entry (Vanilla stores booleans as bytes).
    #[must_use]
    pub fn get_bool(&self, name: &str) -> Option<bool> {
        Some(self.get_i8(name)? != 0)
    }

    /// `f64` value of a compound entry (`Float` accepted and widened).
    #[must_use]
    pub fn get_f64(&self, name: &str) -> Option<f64> {
        self.get(name)?.as_f64()
    }

    /// String value of a compound entry.
    #[must_use]
    pub fn get_str(&self, name: &str) -> Option<&str> {
        match self.get(name)? {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    /// List value of a compound entry.
    #[must_use]
    pub fn get_list(&self, name: &str) -> Option<&[NbtTag]> {
        match self.get(name)? {
            Self::List(items) => Some(items),
            _ => None,
        }
    }

    /// Compound value of a compound entry.
    #[must_use]
    pub fn get_compound(&self, name: &str) -> Option<&NbtTag> {
        match self.get(name)? {
            value @ Self::Compound(_) => Some(value),
            _ => None,
        }
    }

    /// Byte-array value of a compound entry.
    #[must_use]
    pub fn get_byte_array(&self, name: &str) -> Option<&[u8]> {
        match self.get(name)? {
            Self::ByteArray(bytes) => Some(bytes),
            _ => None,
        }
    }

    /// Int-array value of a compound entry.
    #[must_use]
    pub fn get_int_array(&self, name: &str) -> Option<&[i32]> {
        match self.get(name)? {
            Self::IntArray(values) => Some(values),
            _ => None,
        }
    }

    /// Long-array value of a compound entry.
    #[must_use]
    pub fn get_long_array(&self, name: &str) -> Option<&[i64]> {
        match self.get(name)? {
            Self::LongArray(values) => Some(values),
            _ => None,
        }
    }

    /// Number of entries in a compound, or `None` for other tags.
    #[must_use]
    pub fn len(&self) -> Option<usize> {
        self.entries().map(<[_]>::len)
    }

    /// Whether a compound has no entries; `None` for other tags.
    #[must_use]
    pub fn is_empty(&self) -> Option<bool> {
        self.entries().map(<[_]>::is_empty)
    }
}

impl From<&str> for NbtTag {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

impl From<String> for NbtTag {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<i32> for NbtTag {
    fn from(value: i32) -> Self {
        Self::Int(value)
    }
}

impl From<i64> for NbtTag {
    fn from(value: i64) -> Self {
        Self::Long(value)
    }
}

#[cfg(test)]
mod tests {
    use super::{NbtTag, tag};

    #[test]
    fn tag_ids_match_vanilla() {
        assert_eq!(NbtTag::Byte(1).tag_id(), tag::BYTE);
        assert_eq!(NbtTag::Short(1).tag_id(), tag::SHORT);
        assert_eq!(NbtTag::Int(1).tag_id(), tag::INT);
        assert_eq!(NbtTag::Long(1).tag_id(), tag::LONG);
        assert_eq!(NbtTag::Float(1.0).tag_id(), tag::FLOAT);
        assert_eq!(NbtTag::Double(1.0).tag_id(), tag::DOUBLE);
        assert_eq!(NbtTag::ByteArray(vec![1]).tag_id(), tag::BYTE_ARRAY);
        assert_eq!(NbtTag::String("x".to_owned()).tag_id(), tag::STRING);
        assert_eq!(NbtTag::List(vec![]).tag_id(), tag::LIST);
        assert_eq!(NbtTag::Compound(vec![]).tag_id(), tag::COMPOUND);
        assert_eq!(NbtTag::IntArray(vec![1]).tag_id(), tag::INT_ARRAY);
        assert_eq!(NbtTag::LongArray(vec![1]).tag_id(), tag::LONG_ARRAY);
        for id in 0..=12u8 {
            assert_ne!(tag::name(id), "TAG_Unknown", "id {id} needs a name");
        }
    }

    #[test]
    fn lookup_is_last_wins_like_vanilla_put() {
        let compound = NbtTag::Compound(vec![
            ("k".to_owned(), NbtTag::Int(1)),
            ("k".to_owned(), NbtTag::Int(2)),
        ]);
        assert_eq!(compound.get_i32("k"), Some(2));
    }

    #[test]
    fn insert_replaces_in_place_and_drops_duplicates() {
        let mut compound = NbtTag::Compound(vec![
            ("a".to_owned(), NbtTag::Int(1)),
            ("k".to_owned(), NbtTag::Int(2)),
            ("b".to_owned(), NbtTag::Int(3)),
            ("k".to_owned(), NbtTag::Int(4)),
        ]);
        compound.insert("k", NbtTag::Int(9));
        let entries = compound.entries().expect("compound");
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[1].0, "k");
        assert_eq!(entries[1].1, NbtTag::Int(9));
        assert_eq!(compound.get_i32("k"), Some(9));
    }

    #[test]
    fn integer_type_widening_for_tolerant_reads() {
        let compound = NbtTag::Compound(vec![
            ("byte".to_owned(), NbtTag::Byte(-1)),
            ("short".to_owned(), NbtTag::Short(300)),
            ("long".to_owned(), NbtTag::Long(7_000_000_000)),
        ]);
        assert_eq!(compound.get_i32("byte"), Some(-1));
        assert_eq!(compound.get_i32("short"), Some(300));
        assert_eq!(compound.get_i32("long"), None, "out of i32 range");
        assert_eq!(compound.get_i64("long"), Some(7_000_000_000));
        assert_eq!(compound.get_i8("short"), None, "out of i8 range");
    }

    #[test]
    fn bool_and_float_accessors() {
        let compound = NbtTag::Compound(vec![
            ("flag".to_owned(), NbtTag::Byte(1)),
            ("off".to_owned(), NbtTag::Byte(0)),
            ("f".to_owned(), NbtTag::Float(0.5)),
        ]);
        assert_eq!(compound.get_bool("flag"), Some(true));
        assert_eq!(compound.get_bool("off"), Some(false));
        assert_eq!(compound.get_f64("f"), Some(0.5));
    }

    #[test]
    fn accessors_on_non_compound_return_none() {
        let tag = NbtTag::Int(5);
        assert!(tag.entries().is_none());
        assert!(tag.get("x").is_none());
        assert!(tag.get_i32("x").is_none());
        assert_eq!(tag.len(), None);
        let mut not_compound = NbtTag::Int(5);
        not_compound.insert("x", NbtTag::Int(1));
        assert_eq!(not_compound, NbtTag::Int(5), "insert is a no-op");
        assert!(!not_compound.remove("x"));
    }
}
