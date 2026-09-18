//! The filter contract a mod implements over a craft's candidate containers.
//!
//! A filter may only remove candidates. The client predicts its own inventory
//! and no anti-cheat layer reconciles the two, so anything beyond removal puts
//! the two ledgers out of step.
