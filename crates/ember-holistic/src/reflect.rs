//! The reflection vocabulary the binary carries.
//!
//! Every engine type has a descriptor holding its names, size, alignment,
//! fields and enumerators. That is where layout comes from, so no layout is
//! ever recorded in a signature table.

pub mod image_table;
pub mod layout;
pub mod registry;
pub mod type_id;
