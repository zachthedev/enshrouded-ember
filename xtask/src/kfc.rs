//! The revision record in `enshrouded_server.kfc`.
//!
//! The executable formats its revision at runtime, so the image carries the
//! format string and not the number. The data pack beside it carries the number
//! in its header as one NUL-terminated field:
//! `<revision>|<caret path>|<timestamp>`. The caret path is Subversion syntax,
//! because Keen keeps the game content tree in Subversion and the engine tree
//! in git.
//!
//! Reading it fills a signature row's revision column, and cross-checks a build
//! fingerprint, with nothing launched.

use std::fmt;
use std::path::{Path, PathBuf};

/// Bytes of the pack header the record is searched in.
const WINDOW: usize = 512;

/// Shortest run that could hold a revision, a caret path and a timestamp.
const MIN_RECORD: usize = 16;

/// The revision record, as the pack header states it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revision {
    /// The Subversion revision number of the game content tree.
    pub number: u32,
    /// The caret path naming the branch the content came from.
    pub branch: String,
    /// The timestamp the pack was built, as written.
    pub stamp: String,
}

/// Why a pack header yielded no revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevisionError {
    /// The pack is not where the server executable is.
    Missing(PathBuf),
    /// The header ends before the record does.
    Truncated {
        /// Bytes the file actually held.
        read: usize,
    },
    /// The header holds a record that does not parse.
    Malformed {
        /// What the parse expected.
        reason: &'static str,
    },
}

impl fmt::Display for RevisionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RevisionError::Missing(path) => {
                write!(f, "no data pack at {}", path.display())
            }
            RevisionError::Truncated { read } => {
                write!(
                    f,
                    "the pack header ends after {read} bytes, before the revision record"
                )
            }
            RevisionError::Malformed { reason } => {
                write!(f, "the pack header holds no revision record: {reason}")
            }
        }
    }
}

impl std::error::Error for RevisionError {}

/// Read the revision record from the pack beside a server executable.
///
/// # Errors
///
/// Returns [`RevisionError`] naming which of the three ways the read failed.
pub fn read_beside(server_exe: &Path) -> Result<Revision, RevisionError> {
    use std::io::Read as _;

    let pack = server_exe.with_extension("kfc");
    let mut file = std::fs::File::open(&pack).map_err(|_| RevisionError::Missing(pack.clone()))?;
    // Only the header window is read, so a pack of any size costs 512 bytes.
    let mut header = Vec::with_capacity(WINDOW);
    file.by_ref()
        .take(WINDOW as u64)
        .read_to_end(&mut header)
        .map_err(|_| RevisionError::Missing(pack))?;
    parse(&header)
}

/// Parse a pack header.
///
/// # Errors
///
/// Returns [`RevisionError`] when the header is short or holds no record.
pub fn parse(header: &[u8]) -> Result<Revision, RevisionError> {
    let window = &header[..header.len().min(WINDOW)];
    if window.len() < MIN_RECORD {
        return Err(RevisionError::Truncated { read: window.len() });
    }

    let mut best: Option<&'static str> = None;
    let mut start = None;
    for (index, byte) in window.iter().enumerate() {
        let printable = (0x20..=0x7e).contains(byte);
        match (printable, start) {
            (true, None) => start = Some(index),
            (false, Some(from)) => {
                let run = &window[from..index];
                match evaluate(run) {
                    Ok(found) => return Ok(found),
                    Err(Some(reason)) => best = best.or(Some(reason)),
                    Err(None) => {}
                }
                start = None;
            }
            _ => {}
        }
    }
    if let Some(from) = start {
        // A run reaching the end of the window has no terminator inside it, so
        // the record it might hold is cut off rather than absent.
        if window.len() - from >= MIN_RECORD && window[from..].contains(&b'^') {
            return Err(RevisionError::Truncated { read: header.len() });
        }
    }
    Err(RevisionError::Malformed {
        reason: best.unwrap_or("no field carrying a Subversion caret path"),
    })
}

/// Judge one printable run.
///
/// `Err(None)` means the run was never a candidate. `Err(Some(reason))` means
/// it looked like the record and failed, which is worth reporting.
fn evaluate(run: &[u8]) -> Result<Revision, Option<&'static str>> {
    if run.len() < MIN_RECORD {
        return Err(None);
    }
    let Ok(text) = std::str::from_utf8(run) else {
        return Err(None);
    };
    if !text.contains("^/") {
        return Err(None);
    }
    let parts: Vec<&str> = text.split('|').collect();
    let [revision, branch, stamp] = parts.as_slice() else {
        return Err(Some("expected two '|' separators"));
    };
    if revision.is_empty() || !revision.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Some("the revision field is not a number"));
    }
    let Ok(revision) = revision.parse::<u32>() else {
        return Err(Some("the revision field does not fit in 32 bits"));
    };
    if !branch.starts_with("^/") {
        return Err(Some("the branch field is not a Subversion caret path"));
    }
    Ok(Revision {
        number: revision,
        branch: (*branch).to_string(),
        stamp: (*stamp).to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::{Revision, RevisionError, parse};

    /// Wrap a record the way the pack does: binary before it, a NUL after it.
    fn pack(record: &str) -> Vec<u8> {
        let mut bytes = vec![0u8; 144];
        bytes[0..4].copy_from_slice(b"KFC3");
        bytes.extend_from_slice(record.as_bytes());
        bytes.extend_from_slice(&[0u8; 32]);
        bytes
    }

    #[test]
    fn a_well_formed_record_parses() {
        let header = pack("1024233|^/game38/branches/ea_update_08|2026-05-11T10:44:54.050433Z");
        assert_eq!(
            parse(&header),
            Ok(Revision {
                number: 1_024_233,
                branch: "^/game38/branches/ea_update_08".to_string(),
                stamp: "2026-05-11T10:44:54.050433Z".to_string(),
            })
        );
    }

    #[test]
    fn malformed_records_name_what_was_expected() {
        let cases: &[(&str, &str)] = &[
            (
                "1024233;^/game38/branches/ea_update_08;2026-05-11T",
                "expected two '|' separators",
            ),
            (
                "1024233|^/game38/branches/ea_update_08|2026|extra",
                "expected two '|' separators",
            ),
            (
                "r1024233|^/game38/branches/ea_update_08|2026-05-11T",
                "the revision field is not a number",
            ),
            (
                "|^/game38/branches/ea_update_08|2026-05-11T10:44:54Z",
                "the revision field is not a number",
            ),
            (
                "99999999999|^/game38/branches/ea_update_08|2026-05-11T",
                "the revision field does not fit in 32 bits",
            ),
            (
                "1024233|/game38/branches/ea_update_08|2026-05-11T^/x",
                "the branch field is not a Subversion caret path",
            ),
        ];
        for (record, reason) in cases {
            assert_eq!(
                parse(&pack(record)),
                Err(RevisionError::Malformed { reason }),
                "record {record}"
            );
        }
    }

    #[test]
    fn a_header_with_no_caret_path_is_malformed() {
        let header = pack("1024233|game38|2026-05-11T10:44:54Z");
        assert_eq!(
            parse(&header),
            Err(RevisionError::Malformed {
                reason: "no field carrying a Subversion caret path"
            })
        );
    }

    #[test]
    fn a_short_header_is_truncated() {
        let cases: &[&[u8]] = &[b"", b"KFC3", b"KFC3\0RC\0\x0c\0\0\0"];
        for case in cases {
            assert_eq!(
                parse(case),
                Err(RevisionError::Truncated { read: case.len() }),
                "header of {} bytes",
                case.len()
            );
        }
    }

    #[test]
    fn a_record_cut_off_by_the_window_is_truncated() {
        let mut header = vec![0u8; 480];
        header[0..4].copy_from_slice(b"KFC3");
        header.extend_from_slice(b"1024233|^/game38/branches/ea_update_08|2026-05-11T10:44:54Z");
        assert_eq!(
            parse(&header),
            Err(RevisionError::Truncated { read: header.len() })
        );
    }

    #[test]
    fn the_window_bounds_the_search() {
        let mut header = vec![0u8; 512];
        header[0..4].copy_from_slice(b"KFC3");
        header.extend_from_slice(b"1024233|^/game38/branches/ea_update_08|2026-05-11T10:44:54Z\0");
        assert_eq!(
            parse(&header),
            Err(RevisionError::Malformed {
                reason: "no field carrying a Subversion caret path"
            }),
            "a record past the header window is not the pack's revision record"
        );
    }
}
