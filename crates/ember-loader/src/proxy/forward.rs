//! Resolving a slot to the real system library.
//!
//! Slots resolve on first call, because a statically imported function can run
//! before the startup thread is scheduled. Every slot that fails to resolve is
//! logged once.
