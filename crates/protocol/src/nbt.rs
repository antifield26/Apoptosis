//! Network NBT: the subset sent inside packet frames (P02-04, consolidated in
//! P03-01).
//!
//! Phase 02 implemented NBT inside this crate. Phase 03 needed the same model
//! for disk files (`level.dat`, chunk NBT), so the implementation moved to
//! [`mc_nbt`] and this module is now a compatibility façade:
//!
//! - the disk and network encodings differ **only** in whether the root tag
//!   carries a name (network: nameless) and how the bytes are framed;
//! - strings are Java modified UTF-8 with a `u16` prefix in both, verified
//!   against the 26.1.2 runtime (`net.minecraft.nbt.StringTag.write` →
//!   `DataOutput.writeUTF`; measurements in the [`mc_nbt`] crate docs).
//!
//! Keeping one implementation means one hostile-input test suite and no chance
//! of the two encodings drifting apart.

pub use mc_nbt::{NbtTag as Nbt, read_modified_utf8, tag, write_modified_utf8};

#[cfg(test)]
mod tests {
    use super::Nbt;

    #[test]
    fn network_compound_round_trip() {
        let value = Nbt::Compound(vec![
            ("name".to_owned(), Nbt::String("overworld".to_owned())),
            ("height".to_owned(), Nbt::Int(384)),
            ("scale".to_owned(), Nbt::Double(1.0)),
            (
                "flags".to_owned(),
                Nbt::List(vec![Nbt::Byte(1), Nbt::Byte(0)]),
            ),
            (
                "nested".to_owned(),
                Nbt::Compound(vec![("x".to_owned(), Nbt::Long(7))]),
            ),
        ]);
        let mut bytes = Vec::new();
        value.write_network(&mut bytes).expect("encodes");
        let mut slice = &bytes[..];
        let decoded = Nbt::read_network(&mut slice).expect("decodes");
        assert_eq!(decoded, value);
        assert!(slice.is_empty());
        // Nameless root: the first byte is the compound tag id, not a length.
        assert_eq!(bytes[0], super::tag::COMPOUND);
    }

    #[test]
    fn empty_list_uses_end_element_type() {
        let value = Nbt::List(Vec::new());
        let mut bytes = Vec::new();
        value.write_network(&mut bytes).expect("encodes");
        assert_eq!(bytes, [super::tag::LIST, super::tag::END, 0, 0, 0, 0]);
    }

    #[test]
    fn hostile_lengths_are_rejected() {
        // Compound { "l": List<Byte> claiming 1_000_000 elements, none present }.
        let bytes = [
            super::tag::COMPOUND,
            super::tag::LIST,
            0x00,
            0x01,
            b'l',
            super::tag::BYTE,
            0x00,
            0x0F,
            0x42,
            0x40,
        ];
        let mut slice = &bytes[..];
        assert!(Nbt::read_network(&mut slice).is_err());
    }

    #[test]
    fn unknown_tag_id_is_rejected() {
        let mut slice = &[0x63u8][..];
        assert!(Nbt::read_network(&mut slice).is_err());
    }
}
