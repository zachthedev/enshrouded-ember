//! A mod's manifest.
//!
//! It is compiled into the library and returned by the register export, so it
//! cannot drift from the binary it describes. A mod id becomes a directory
//! name, which is why it is bounded and carries no path separator.
