//! The surface a mod author writes against.
//!
//! A mod is a dynamic library exporting the C symbols [`declare`] describes.
//! Ember resolves the build, loads each mod in dependency order, and hands it
//! the surfaces it asked for. A mod whose required symbols are missing is disabled
//! by name in the log while the server keeps running.
//!
//! This crate owns the mod lifecycle and re-exports the engine and game
//! surfaces. It holds no knowledge of either itself.

pub mod abi;
pub mod config;
pub mod declare;
pub mod events;
pub mod hooks;
pub mod lifecycle;
pub mod log;
pub mod manifest;
pub mod symbols;

pub use ember_enshrouded as game;
pub use ember_holistic as engine;
