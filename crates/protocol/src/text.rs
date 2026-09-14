//! Minimal text components for disconnect reasons.
//!
//! Wire rules observed in the 26.1 references:
//! - login disconnect uses a JSON string (`LoginDisconnectPacket` `JSON_COMPONENT`);
//! - configuration/play disconnect use an NBT text component
//!   (`DisconnectPacket` COMPONENT).
//!
//! Phase 02 only needs literal text; styled/translated components arrive with
//! the chat work in later phases.

use crate::nbt::Nbt;

/// A server-authored text component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextComponent {
    /// Plain literal text.
    Literal(String),
}

impl TextComponent {
    /// Create a literal component.
    #[must_use]
    pub fn literal(text: impl Into<String>) -> Self {
        Self::Literal(text.into())
    }

    /// The plain text without styling.
    #[must_use]
    pub fn as_plain(&self) -> &str {
        match self {
            Self::Literal(text) => text,
        }
    }

    /// Render as a network-NBT text component.
    ///
    /// **A plain literal is a bare `TAG_String`** — what a real 26.1.2 server sends
    /// (`08 00 03 'b','y','e'` for "bye"); the compound `{ text: "…" }` form this
    /// function used to write is rejected by a real client's component decoder,
    /// which kicked a joined real client with a `DecoderException` on
    /// `disguised_chat` (found in acceptance, P10-11). The byte-identity note in
    /// `disguised_chat_golden.rs` documents the same gap and must be updated with
    /// this fix.
    #[must_use]
    pub fn to_nbt(&self) -> Nbt {
        Nbt::String(self.as_plain().to_owned())
    }

    /// Render as a JSON component (`{"text":"..."}`) for login disconnects and
    /// status responses.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::json!({ "text": self.as_plain() }).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::TextComponent;

    #[test]
    fn json_escapes_control_characters() {
        let component = TextComponent::literal("a\"b\\c\n");
        let json = component.to_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(parsed["text"], "a\"b\\c\n");
    }

    #[test]
    fn nbt_shape_is_a_bare_string_like_a_real_server_sends() {
        // The accepted shape on this protocol: a real 26.1.2 server sends a plain
        // message as a bare TAG_String, and a real client rejects the compound
        // {text: ...} form (acceptance finding, P10-11).
        assert_eq!(
            TextComponent::literal("bye").to_nbt(),
            crate::nbt::Nbt::String("bye".to_owned())
        );
    }
}
