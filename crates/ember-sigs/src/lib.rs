//! Per-build signature and layout tables.
//!
//! Keen ships no debugging symbols, so every address Ember needs is recovered at
//! runtime in three tiers: a cross-reference from one of the binary's own
//! registered names, a byte pattern with wildcards, and a recorded offset as the
//! lock. This crate holds the recorded tier, as data.
//!
//! Rows key on the platform and the build revision the server prints at startup
//! as `Game Version (SVN): N`, cross-checked against the executable timestamp.
//! The four-part version string does not identify a build: two different
//! binaries shipped as v0.9.1.2.
//!
//! Resolution fails closed. A missing required symbol disables the mod that
//! needed it, names the symbol in the log, and leaves the server running vanilla
//! for that feature.
