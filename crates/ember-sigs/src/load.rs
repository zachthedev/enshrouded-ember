//! Reading a signature table out of TOML.
//!
//! A table that contradicts itself is refused at load, so a disagreement
//! surfaces before the loader picks a row.
