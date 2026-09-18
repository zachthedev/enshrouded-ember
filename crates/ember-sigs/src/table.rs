//! The signature table format.
//!
//! A `Build` row keyed on a fingerprint, carrying the git sha and the revision,
//! with the symbol rows under it. A row holds function addresses and the
//! patterns that recover them. Nothing about a type ever appears in one.
