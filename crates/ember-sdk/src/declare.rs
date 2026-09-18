//! The macro that exports the two C symbols a mod library must carry.
//!
//! Registering returns the manifest and nothing else. Loading runs later,
//! through the descriptor, once the host has ordered mods and checked what each
//! one requires.
