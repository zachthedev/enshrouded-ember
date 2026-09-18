//! The startup report: which build, which symbols, which mods, which links.
//!
//! It names which mod holds which position on which target, so a link that
//! stops a call is attributable. It also states that any crash dump taken with
//! Ember loaded has a wrong stack through hooked frames.
