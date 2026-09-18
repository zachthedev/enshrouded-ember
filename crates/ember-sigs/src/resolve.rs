//! Resolving a symbol id to an address.
//!
//! Three tiers run: a cross-reference from a format string, a wildcard byte
//! pattern, then the recorded offset as the lock. Every tier that runs must
//! agree with the recorded offset, because a contradiction means the build
//! moved.
