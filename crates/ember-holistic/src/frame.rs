//! `Frame`, the token that makes engine memory safe to read.
//!
//! A frame is obtained inside a hook body, where the engine is known to be at a
//! point it is not mutating storage. Every borrow into engine memory is tied to
//! its lifetime.
