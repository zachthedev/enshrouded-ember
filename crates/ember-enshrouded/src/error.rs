//! The error type every fallible operation in this crate returns.
//!
//! The game sits above the engine, so an operation here can fail for a reason
//! of its own or for one the engine raised. `Engine` carries the second kind
//! through unchanged, which keeps one type on every signature in the crate
//! while losing nothing a caller could have matched on.
//!
//! Every variant names the symbol, feature or operation that failed, so a
//! fail-closed log line is complete without the caller adding context.

/// The result of an operation against the server.
pub type Result<T> = core::result::Result<T, GameError>;

/// Why an operation against the server refused.
#[derive(Debug, thiserror::Error)]
pub enum GameError {
    /// The signature table resolves this symbol on no build in the supported
    /// window.
    #[error("required symbol `{name}` did not resolve")]
    SymbolMissing {
        /// The registered name that was looked up.
        name: &'static str,
    },

    /// The symbol resolved, and what sits at the address is not the shape this
    /// build expects.
    #[error("`{name}` resolved but its shape did not match: {detail}")]
    ShapeMismatch {
        /// The registered name that was looked up.
        name: &'static str,
        /// Which part of the shape disagreed.
        detail: String,
    },

    /// A decode refused because its input is shorter than the layout needs.
    ///
    /// A decoder reads fields at fixed offsets, so a short input reads past
    /// the end rather than failing on its own. This resolves nothing and names
    /// no symbol, which is what separates it from `ShapeMismatch`.
    #[error("decoding {what} needs {expected} bytes, the input holds {found}")]
    Decode {
        /// What was being decoded.
        what: &'static str,
        /// How many bytes the input holds.
        found: usize,
        /// How many bytes the layout needs.
        expected: usize,
    },

    /// The feature exists in the game and not on a dedicated server.
    #[error("`{feature}` is not available on a dedicated server: {reason}")]
    Unavailable {
        /// The feature that was asked for.
        feature: &'static str,
        /// Why a dedicated server does not carry it.
        reason: &'static str,
    },

    /// The operation is available and cannot represent what this input asks
    /// for.
    ///
    /// `Unavailable` is about the server lacking a feature. This is about an
    /// input the operation cannot express, such as a filter whose decision
    /// depends on a recipe being converted into one that never sees a recipe.
    #[error("`{op}` cannot accept this input: {reason}")]
    Unsupported {
        /// The operation that was attempted.
        op: &'static str,
        /// Why this input cannot be expressed.
        reason: &'static str,
    },

    /// The server is running, and not in the state this operation needs.
    #[error("the server is not in a state where `{op}` is legal")]
    WrongPhase {
        /// The operation that was attempted.
        op: &'static str,
    },

    /// The engine refused underneath.
    #[error(transparent)]
    Engine(#[from] ember_holistic::HolisticError),
}

#[cfg(test)]
mod tests {
    use super::GameError;

    /// Every variant's name, as the exhaustive match below spells it.
    ///
    /// The match carries no wildcard arm, so adding a variant stops this file
    /// compiling and brings whoever added it here.
    const VARIANTS: [&str; 7] = [
        "SymbolMissing",
        "ShapeMismatch",
        "Decode",
        "Unavailable",
        "Unsupported",
        "WrongPhase",
        "Engine",
    ];

    /// Which variant a value is.
    fn variant_of(error: &GameError) -> &'static str {
        match error {
            GameError::SymbolMissing { .. } => "SymbolMissing",
            GameError::ShapeMismatch { .. } => "ShapeMismatch",
            GameError::Decode { .. } => "Decode",
            GameError::Unavailable { .. } => "Unavailable",
            GameError::Unsupported { .. } => "Unsupported",
            GameError::WrongPhase { .. } => "WrongPhase",
            GameError::Engine(..) => "Engine",
        }
    }

    /// One value per variant, with the substrings its message must carry.
    fn cases() -> Vec<(GameError, Vec<String>)> {
        vec![
            (
                GameError::SymbolMissing {
                    name: "enshrouded.chat.send",
                },
                vec!["enshrouded.chat.send".to_string()],
            ),
            (
                GameError::ShapeMismatch {
                    name: "enshrouded.save.step",
                    detail: "fourth argument is not a byte".to_string(),
                },
                vec![
                    "enshrouded.save.step".to_string(),
                    "fourth argument is not a byte".to_string(),
                ],
            ),
            (
                GameError::Decode {
                    what: "ItemStack",
                    found: 7,
                    expected: 16,
                },
                vec!["ItemStack".to_string(), "7".to_string(), "16".to_string()],
            ),
            (
                GameError::Unavailable {
                    feature: "chat",
                    reason: "enableTextChat is false",
                },
                vec!["chat".to_string(), "enableTextChat is false".to_string()],
            ),
            (
                GameError::Unsupported {
                    op: "from_crafting_filter",
                    reason: "the filter's decision depends on the recipe",
                },
                vec![
                    "from_crafting_filter".to_string(),
                    "the filter's decision depends on the recipe".to_string(),
                ],
            ),
            (
                GameError::WrongPhase { op: "current_slot" },
                vec!["current_slot".to_string()],
            ),
            (
                GameError::Engine(ember_holistic::HolisticError::ComponentAbsent(
                    "InventorySetup",
                )),
                vec!["InventorySetup".to_string()],
            ),
        ]
    }

    /// Every variant round-trips through `Display` with what failed present.
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

    /// `Engine` is transparent, so the engine's own message is what a caller
    /// reads rather than a wrapper naming the game crate.
    #[test]
    fn the_engine_arm_renders_the_engine_message() {
        let inner = ember_holistic::HolisticError::StaleHandle;
        let wrapped = GameError::Engine(ember_holistic::HolisticError::StaleHandle);

        assert_eq!(wrapped.to_string(), inner.to_string());
    }
}
