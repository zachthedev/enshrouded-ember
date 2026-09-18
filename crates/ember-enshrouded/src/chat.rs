//! Text chat, in both directions.
//!
//! Chat is the one channel confirmed to carry free text to a player, and the
//! server ships it switched off, so every caller tolerates its absence.

pub mod inbound;
pub mod outbound;
