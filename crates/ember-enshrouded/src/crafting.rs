//! Crafting: which containers feed a station, and what a craft consumes.
//!
//! A craft resolves its materials at consumption time against live inventories,
//! so there is no stock figure to read anywhere.

pub mod exec_filter;
pub mod filter;
pub mod props;
pub mod replication_filter;
pub mod stock;
