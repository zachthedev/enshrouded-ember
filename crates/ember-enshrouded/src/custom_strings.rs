//! The server-held table of free player text.
//!
//! The receive path checks nothing about the sender, so any connected client
//! can set any entity's string. No caller may treat one as evidence.
