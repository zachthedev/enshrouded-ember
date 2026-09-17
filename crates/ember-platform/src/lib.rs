//! Operating system seam for Ember.
//!
//! Three concerns cross the operating system boundary and nothing else does:
//! reading the running executable's image, installing an inline hook, and
//! getting a library loaded into the server process. Each is a trait with a
//! backend per platform, so the rest of Ember and every mod stay platform
//! neutral.
//!
//! Keen ships a Windows dedicated server only, and the Linux backends are
//! declared rather than written against a binary that does not exist.

/// The operating system a server binary was built for.
///
/// Signature tables key on this together with the build revision, because one
/// revision can ship as more than one binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Platform {
    /// `enshrouded_server.exe`, running on Windows or under Wine or Proton.
    Windows,
    /// A native Linux server binary.
    Linux,
}

impl Platform {
    /// The platform this build of Ember runs on.
    #[must_use]
    pub const fn host() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Linux
        }
    }

    /// The short name signature tables and log lines use.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Linux => "linux",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Platform;

    #[test]
    fn names_are_the_table_keys() {
        assert_eq!(Platform::Windows.as_str(), "windows");
        assert_eq!(Platform::Linux.as_str(), "linux");
    }

    #[test]
    fn host_matches_the_compilation_target() {
        let expected = if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Linux
        };
        assert_eq!(Platform::host(), expected);
    }
}
