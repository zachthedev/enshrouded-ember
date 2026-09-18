//! Reading a mod's own configuration file.
//!
//! A missing file is written from defaults with every field present. A file
//! that fails to parse is never overwritten. An unknown key disables the mod by
//! name.
