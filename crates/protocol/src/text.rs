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

    /// Render as a network-NBT text component (`{ text: "..." }`).
    #[must_use]
    pub fn to_nbt(&self) -> Nbt {
        Nbt::Compound(vec![(
            "text".to_owned(),
            Nbt::String(self.as_plain().to_owned()),
        )])
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
    fn nbt_shape_is_literal_text() {
        let nbt = TextComponent::literal("bye").to_nbt();
        let crate::nbt::Nbt::Compound(entries) = nbt else {
            panic!("expected compound");
        };
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, "text");
    }
}
