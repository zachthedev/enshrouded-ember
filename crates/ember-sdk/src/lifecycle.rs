//! The `Mod` trait and the context each of its stages receives.
//!
//! Hook and event registration is allowed while loading and refused after the
//! host arms the chain, so the set of links is fixed for the life of the
//! process.
