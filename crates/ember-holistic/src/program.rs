//! Programs, which is what the engine calls its systems.
//!
//! One record in read-only data carries a program's name, its entry function
//! and the components it triggers on, reads and writes. The entry is null when
//! a build ships the program's name without its code.
