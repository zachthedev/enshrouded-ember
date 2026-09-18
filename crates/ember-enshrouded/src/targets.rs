//! The one list of hook targets the loader installs.
//!
//! It is live only in the loader's copy of this crate. A mod library carries
//! its own dead copy, so a mod that registers here registers into nothing.
