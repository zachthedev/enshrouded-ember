//! The two ways Ember starts: the library entry point, and the injected one.
//!
//! The library entry records the module handle, spawns a thread and returns,
//! because installing a hook under the loader lock suspends threads and
//! deadlocks. The injected entry runs the same body on the calling thread.
