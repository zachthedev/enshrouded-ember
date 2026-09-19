//! Bindings to the Enshrouded server.
//!
//! Everything here is true because the game is Enshrouded rather than because
//! the engine is Holistic: player sessions, chat and heads-up notifications,
//! save cycles, items, recipes, and crafting stock.
//!
//! One public game runs on this engine, so nothing can test cross-game reuse.
//! Keeping the game surface here means `ember-holistic` stays engine shaped.

pub mod anchors;
pub mod area_deposit;
pub mod chat;
pub mod config;
pub mod crafting;
pub mod custom_strings;
pub mod error;
pub mod interaction;
pub mod inventory;
pub mod items;
pub mod placement;
pub mod recipes;
pub mod save;
pub mod session;
pub mod targets;
pub mod ui;

pub use error::{GameError, Result};
