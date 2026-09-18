//! The implementation behind every host table entry, and the context it reads.
//!
//! The host hands out addresses, offsets and descriptors. It never proxies a
//! read of engine memory, which is what keeps the table small enough to freeze.
