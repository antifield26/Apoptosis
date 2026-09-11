//! Stable identifier primitives (P01-04).
//!
//! Minecraft namespaces names (`minecraft:overworld`, `minecraft:stone`) show
//! up in registries, datapacks and the protocol. Parsing them once, strictly,
//! keeps every later crate from re-implementing ad-hoc validation.

/// A validated `namespace:value` resource identifier.
///
/// Rules: both parts non-empty, lowercase ASCII alphanumeric plus
/// `._-/` in the value and `._-` in the namespace. Additionally, `.` and `..`
/// path segments are rejected so an id can never smuggle directory traversal
/// past validation (AGENTS.md section 10 threat list). Anything else is
/// rejected so hostile input fails at the boundary with
/// [`crate::error::ServerError::Protocol`].
///
/// `Ord` is derived so ids can key ordered maps: deterministic iteration is a
/// precondition for reproducible saves and traces (AGENTS.md section 3.6).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResourceId {
    namespace: String,
    value: String,
}

impl ResourceId {
    /// Parse and validate `namespace:value`; bare `value` defaults to the
    /// `minecraft` namespace, matching vanilla behaviour.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::ServerError::Protocol`] when the input is empty
    /// or contains characters outside the allowed sets.
    pub fn parse(input: &str) -> crate::error::ServerResult<Self> {
        fn valid_namespace(s: &str) -> bool {
            !s.is_empty()
                && s != "."
                && s != ".."
                && s.bytes().all(|b| {
                    b.is_ascii_lowercase()
                        || b.is_ascii_digit()
                        || b == b'.'
                        || b == b'_'
                        || b == b'-'
                })
        }

        fn valid_value(s: &str) -> bool {
            !s.is_empty()
                && !s.split('/').any(|seg| seg == "." || seg == "..")
                && s.bytes().all(|b| {
                    b.is_ascii_lowercase()
                        || b.is_ascii_digit()
                        || b == b'.'
                        || b == b'_'
                        || b == b'-'
                        || b == b'/'
                })
        }

        let (namespace, value) = match input.split_once(':') {
            Some((ns, v)) => (ns, v),
            None => ("minecraft", input),
        };
        if value.starts_with('/') || value.ends_with('/') || value.contains("//") {
            return Err(crate::error::ServerError::Protocol(format!(
                "invalid resource id: {input:?}"
            )));
        }
        if !valid_namespace(namespace) || !valid_value(value) {
            return Err(crate::error::ServerError::Protocol(format!(
                "invalid resource id: {input:?}"
            )));
        }
        Ok(Self {
            namespace: namespace.to_owned(),
            value: value.to_owned(),
        })
    }

    /// The namespace part (e.g. `minecraft`).
    #[must_use]
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// The value part (e.g. `overworld`).
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }
}

impl core::fmt::Display for ResourceId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}:{}", self.namespace, self.value)
    }
}

#[cfg(test)]
mod tests {
    use super::ResourceId;

    #[test]
    fn bare_value_defaults_to_minecraft_namespace() {
        let id = ResourceId::parse("stone").expect("valid id");
        assert_eq!(id.namespace(), "minecraft");
        assert_eq!(id.value(), "stone");
        assert_eq!(id.to_string(), "minecraft:stone");
    }

    #[test]
    fn rejects_hostile_input() {
        for bad in [
            "",
            ":",
            "minecraft:",
            ":stone",
            "MINECRAFT:stone",
            "../etc/passwd",
            "a:b:c",
            "a b",
        ] {
            assert!(ResourceId::parse(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn rejects_path_traversal_payloads() {
        // File/path traversal is in the AGENTS.md section 10 threat list; ids
        // must never smuggle separators or parent segments past validation.
        for bad in ["minecraft:..", "minecraft:a/../b", "minecraft:/abs", "..:x"] {
            let parsed = ResourceId::parse(bad);
            assert!(parsed.is_err(), "should reject {bad:?}");
        }
    }
}
