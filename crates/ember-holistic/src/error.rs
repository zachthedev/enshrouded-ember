//! The error type every fallible operation in this crate returns.
//!
//! One enum covers the whole crate because a caller reading engine memory
//! crosses reflection, layout and entity storage in a single operation, and
//! three error types would make it convert twice on the way out.
//!
//! Every variant names the thing that failed, so a fail-closed log line is
//! complete without the caller adding context. A refusal is always preferred to
//! a plausible answer: the engine's own failures are silent ones, and a field
//! offset that is one record out still reads.

use crate::reflect::type_id::TypeId;

/// The result of an operation against the engine.
pub type Result<T> = core::result::Result<T, HolisticError>;

/// Why an operation against the engine refused.
#[derive(Debug, thiserror::Error)]
pub enum HolisticError {
    /// The image carries no build identity block, or one too incomplete to
    /// trust.
    #[error("build identity not found in the image: {0}")]
    BuildIdentity(&'static str),

    /// The descriptor table holds no type of this name.
    #[error("no reflection descriptor named {0}")]
    UnknownType(String),

    /// An anchor resolved to a number of references other than the one the
    /// caller requires, so which one it names is a guess.
    #[error("anchor {anchor} has {found} references, expected exactly {expected}")]
    AmbiguousAnchor {
        /// The name that was looked up.
        anchor: String,
        /// How many references the scan found.
        found: usize,
        /// How many the caller requires.
        expected: usize,
    },

    /// The entity carries no component of this type.
    #[error("component {0} is not present on this entity")]
    ComponentAbsent(&'static str),

    /// The entity map holds no live entity for this handle.
    ///
    /// A handle is a bare counter value with no generation, so a number the
    /// map no longer holds is the only staleness this crate can observe.
    #[error("entity handle is stale")]
    StaleHandle,

    /// The signature table resolves this symbol on no build in the supported
    /// window.
    #[error("required symbol {0} is missing on this build")]
    MissingSymbol(&'static str),

    /// The type's descriptor declares no field of this name.
    ///
    /// Offsets come from the descriptor's field array and nowhere else, so a
    /// name the array does not carry has no offset to fall back to.
    #[error("{type_name} declares no field named {field}")]
    UnknownField {
        /// The bare type name the layout was taken for.
        type_name: String,
        /// The field name that was asked for.
        field: String,
    },

    /// A buffer is not the length the component's descriptor declares.
    ///
    /// A read decodes fields at recorded offsets and a write lands bytes at
    /// them, so a buffer of the wrong length reads or writes past the end of
    /// the component rather than failing on its own.
    #[error("{component} needs {expected} bytes, the buffer holds {found}")]
    SizeMismatch {
        /// The component the buffer was for.
        component: &'static str,
        /// How many bytes the buffer holds.
        found: usize,
        /// How many bytes the descriptor declares.
        expected: usize,
    },

    /// A read from inside a system job named a component the system's own
    /// access set does not declare.
    ///
    /// Two systems share a stage only when their declared component sets do
    /// not conflict, so an undeclared read races whichever system of the same
    /// stage writes that component.
    #[error(
        "component {:#010x} is not in this system's access set",
        .component.0
    )]
    Undeclared {
        /// The component the read named.
        component: TypeId,
    },
}

#[cfg(test)]
mod tests {
    use super::{HolisticError, TypeId};

    /// Every variant's name, as the exhaustive match below spells it.
    ///
    /// The match carries no wildcard arm, so adding a variant stops this file
    /// compiling and brings whoever added it here.
    const VARIANTS: [&str; 9] = [
        "BuildIdentity",
        "UnknownType",
        "AmbiguousAnchor",
        "ComponentAbsent",
        "StaleHandle",
        "MissingSymbol",
        "UnknownField",
        "SizeMismatch",
        "Undeclared",
    ];

    /// Which variant a value is.
    fn variant_of(error: &HolisticError) -> &'static str {
        match error {
            HolisticError::BuildIdentity(..) => "BuildIdentity",
            HolisticError::UnknownType(..) => "UnknownType",
            HolisticError::AmbiguousAnchor { .. } => "AmbiguousAnchor",
            HolisticError::ComponentAbsent(..) => "ComponentAbsent",
            HolisticError::StaleHandle => "StaleHandle",
            HolisticError::MissingSymbol(..) => "MissingSymbol",
            HolisticError::UnknownField { .. } => "UnknownField",
            HolisticError::SizeMismatch { .. } => "SizeMismatch",
            HolisticError::Undeclared { .. } => "Undeclared",
        }
    }

    /// One value per variant, with the substrings its message must carry.
    fn cases() -> Vec<(HolisticError, Vec<String>)> {
        vec![
            (
                HolisticError::BuildIdentity("no 40-byte hex run in .rdata"),
                vec!["no 40-byte hex run in .rdata".to_string()],
            ),
            (
                HolisticError::UnknownType("keen::ecs::Nope".to_string()),
                vec!["keen::ecs::Nope".to_string()],
            ),
            (
                HolisticError::AmbiguousAnchor {
                    anchor: "cache_crafting_stock".to_string(),
                    found: 3,
                    expected: 1,
                },
                vec![
                    "cache_crafting_stock".to_string(),
                    "3".to_string(),
                    "1".to_string(),
                ],
            ),
            (
                HolisticError::ComponentAbsent("InventorySetup"),
                vec!["InventorySetup".to_string()],
            ),
            (HolisticError::StaleHandle, vec!["stale".to_string()]),
            (
                HolisticError::MissingSymbol("enshrouded.save.step"),
                vec!["enshrouded.save.step".to_string()],
            ),
            (
                HolisticError::UnknownField {
                    type_name: "InventorySetup".to_string(),
                    field: "nope".to_string(),
                },
                vec!["InventorySetup".to_string(), "nope".to_string()],
            ),
            (
                HolisticError::SizeMismatch {
                    component: "InventorySetup",
                    found: 12,
                    expected: 84,
                },
                vec![
                    "InventorySetup".to_string(),
                    "12".to_string(),
                    "84".to_string(),
                ],
            ),
            (
                HolisticError::Undeclared {
                    component: TypeId::component("Inventory"),
                },
                vec!["0xb19528a9".to_string()],
            ),
        ]
    }

    /// Every variant renders a message naming the thing that failed.
    ///
    /// The message is the whole failure surface a mod author sees, so a field
    /// dropped from a format string is otherwise silent.
    #[test]
    fn every_variant_names_what_failed() {
        for (error, wanted) in cases() {
            let rendered: String = error.to_string();
            for needle in wanted {
                assert!(
                    rendered.contains(&needle),
                    "{}: {rendered:?} does not carry {needle:?}",
                    variant_of(&error)
                );
            }
        }
    }

    /// The table covers every variant the enum declares.
    #[test]
    fn the_table_covers_every_variant() {
        let mut covered: Vec<&'static str> =
            cases().iter().map(|(error, _)| variant_of(error)).collect();
        covered.sort_unstable();
        covered.dedup();

        let mut declared: Vec<&'static str> = VARIANTS.to_vec();
        declared.sort_unstable();

        assert_eq!(covered, declared);
    }
}
