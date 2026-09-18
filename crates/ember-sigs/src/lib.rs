//! Per-build signature and layout tables.
//!
//! Keen ships no debugging symbols, so every address Ember needs is recovered at
//! runtime in three tiers: a cross-reference from one of the names the binary
//! registers, a byte pattern with wildcards, and a recorded offset as the lock.
//! This crate holds the recorded tier, as data.
//!
//! Rows key on the platform and the build revision, and carry a fingerprint
//! taken from the executable image. The fingerprint is what the loader matches
//! on, because the revision reaches the log only later in startup, after Ember
//! has already had to choose a row. The revision stays in the row so a human
//! reading the table knows which build it describes.
//!
//! A four-part version string identifies nothing on its own: two different
//! binaries shipped as v0.9.1.2.
//!
//! Resolution fails closed. A missing required symbol disables the mod that
//! needed it, names the symbol in the log, and leaves the server running vanilla
//! for that feature.
