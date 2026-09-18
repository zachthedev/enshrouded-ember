//! The boundary between the loader and a mod library.
//!
//! Every type here is `repr(C)` and its layout is frozen by the version gate in
//! front of it. The two tables each open with a size field, so the host can grow
//! one and an older mod reading the prefix still works.
