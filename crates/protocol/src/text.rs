//! Minimal text components for disconnect reasons.
//!
//! Wire rules observed in the 26.1 references:
//! - login disconnect uses a JSON string (`LoginDisconnectPacket` `JSON_COMPONENT`);
//! - configuration/play disconnect use an NBT text component
//!   (`DisconnectPacket` COMPONENT).
//!
//! Two shapes are written: a **literal**, and a **translatable** component
//! carrying a Vanilla translation key (AUDIT-19 C19-L7 — a client-visible
//! refusal must be a key its own language file resolves, not an English
//! sentence we chose, because the client renders the key in the player's
//! locale). Translatable components carry a `fallback` because that is a real
//! field of Vanilla's component codec (a client without the key shows it) and
//! because it gives the server one readable string to log: [`as_plain`]
//! answers with it.
//!
//! Styled components (colours, click events, nested siblings) are still out of
//! scope; nothing here needs them yet.
//!
//! [`as_plain`]: TextComponent::as_plain

use crate::nbt::Nbt;

/// A server-authored text component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextComponent {
    /// Plain literal text.
    Literal(String),
    /// A Vanilla translation key, with the text a client shows when its
    /// language file has no entry for it.
    ///
    /// The key is sent as-is: `multiplayer.disconnect.unverified_username`
    /// renders "Failed to verify username!" on an English client (the exact
    /// sentence the old hard-coded literal produced) and the player's own
    /// language everywhere else — which is the whole point (AUDIT-19 C19-L7).
    Translatable {
        /// The translation key, e.g. `multiplayer.disconnect.authservers_down`.
        key: String,
        /// English text for the log line and for clients without the key.
        fallback: String,
    },
}

impl TextComponent {
    /// Create a literal component.
    #[must_use]
    pub fn literal(text: impl Into<String>) -> Self {
        Self::Literal(text.into())
    }

    /// Create a translatable component (AUDIT-19 C19-L7).
    ///
    /// `key` is a Vanilla language key and `fallback` the English text for
    /// logs and for a client whose language file lacks the key. The fallback
    /// must be what the key renders to in `en_us`, so a log line and the
    /// player's screen cannot disagree.
    #[must_use]
    pub fn translatable(key: impl Into<String>, fallback: impl Into<String>) -> Self {
        Self::Translatable {
            key: key.into(),
            fallback: fallback.into(),
        }
    }

    /// The plain text without styling.
    ///
    /// A translatable component answers with its fallback: the key is an
    /// opaque identifier, so the sentence is what a log line, a test and an
    /// error message can show.
    #[must_use]
    pub fn as_plain(&self) -> &str {
        match self {
            Self::Literal(text) => text,
            Self::Translatable { fallback, .. } => fallback,
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
    ///
    /// A translatable component is the compound form, because that is the only
    /// shape that can carry a key; the bare-string shortcut above is a literal
    /// encoding and cannot express one.
    #[must_use]
    pub fn to_nbt(&self) -> Nbt {
        match self {
            Self::Literal(text) => Nbt::String(text.clone()),
            Self::Translatable { key, fallback } => Nbt::Compound(vec![
                ("translate".to_owned(), Nbt::String(key.clone())),
                ("fallback".to_owned(), Nbt::String(fallback.clone())),
            ]),
        }
    }

    /// Render as a JSON component for login disconnects and status responses:
    /// `{"text":"..."}` for a literal, `{"translate":"...","fallback":"..."}`
    /// for a translation key.
    #[must_use]
    pub fn to_json(&self) -> String {
        match self {
            Self::Literal(text) => serde_json::json!({ "text": text }).to_string(),
            Self::Translatable { key, fallback } => {
                serde_json::json!({ "translate": key, "fallback": fallback }).to_string()
            }
        }
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

    #[test]
    fn a_translation_key_rides_the_component_instead_of_a_sentence() {
        // AUDIT-19 C19-L7: the client resolves the key against its own
        // language file; an English sentence we chose is not translatable.
        // The fallback is the English sentence the key renders to, so the log
        // line and the old behaviour survive.
        let refuse = TextComponent::translatable(
            "multiplayer.disconnect.unverified_username",
            "Failed to verify username!",
        );
        assert_eq!(refuse.as_plain(), "Failed to verify username!");
        let json: serde_json::Value = serde_json::from_str(&refuse.to_json()).expect("valid JSON");
        assert_eq!(
            json["translate"],
            "multiplayer.disconnect.unverified_username"
        );
        assert_eq!(json["fallback"], "Failed to verify username!");
        assert!(
            json.get("text").is_none(),
            "a translatable component must not masquerade as a literal: {json}"
        );
        let nbt = refuse.to_nbt();
        assert_eq!(
            nbt.get("translate"),
            Some(&crate::nbt::Nbt::String(
                "multiplayer.disconnect.unverified_username".to_owned()
            ))
        );
        // The literal arm is unchanged: still a bare string, never a compound.
        assert_eq!(
            TextComponent::literal("bye").to_nbt(),
            crate::nbt::Nbt::String("bye".to_owned())
        );
    }

    #[test]
    fn the_translatable_nbt_form_round_trips_through_the_wire_codec() {
        // The disconnect packet writes `to_nbt()`; prove the compound survives
        // the network encoding a client decodes, not just our own struct.
        let component = TextComponent::translatable(
            "multiplayer.disconnect.authservers_down",
            "Authentication servers are unavailable",
        );
        let mut bytes = Vec::new();
        component
            .to_nbt()
            .write_network(&mut bytes)
            .expect("encodes");
        let mut slice = &bytes[..];
        let decoded = crate::nbt::Nbt::read_network(&mut slice).expect("decodes");
        assert_eq!(decoded, component.to_nbt());
        assert_eq!(
            decoded.get("translate"),
            component.to_nbt().get("translate")
        );
    }
}
