//! The startup sequence, from resolving the server directory to returning the
//! report.
//!
//! Every path Ember resolves comes from the running executable, never from this
//! library's own location, because under injection it sits in a build
//! directory.
