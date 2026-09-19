//! The macro that exports the C symbols a mod library must carry: the ABI
//! version, which the host reads first, and the registration call that returns
//! the manifest.
//!
//! Registering returns the manifest and nothing else. Loading runs later,
//! through the descriptor, once the host has ordered mods and checked what each
//! one requires.
