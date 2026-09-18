//! Asking the host for an address, and declaring which symbols a mod requires.
//!
//! Resolution happens before a mod loads, so a mod that reaches this point has
//! every symbol it declared as required.
