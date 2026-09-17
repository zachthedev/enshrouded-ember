//! Bindings to the Enshrouded server.
//!
//! Everything here is true because the game is Enshrouded rather than because
//! the engine is Holistic: player sessions, chat and heads-up notifications,
//! save cycles, items, recipes, and crafting stock.
//!
//! One public game runs on this engine, so nothing can test cross-game reuse.
//! Keeping the game surface here means `ember-holistic` stays engine shaped.
