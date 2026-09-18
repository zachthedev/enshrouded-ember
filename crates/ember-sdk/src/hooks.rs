//! Registering a link on a hook target from inside a mod.
//!
//! A Rust closure is wrapped in a C trampoline and handed to the host, which
//! resolves the target and adds the link at the mod's load position.
