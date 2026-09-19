//! The record an extraction writes beside its dumps.
//!
//! `build.json` exists so that nothing has to reopen the server image to learn
//! which build a directory of dumps came from. `schema list` and `schema diff`
//! read it, and the build watcher can read it without a Keen binary on hand.
//!
//! It is derived data under `.cache` like everything else here, so it is never
//! committed. It carries identifiers and timestamps only: no type, no string
//! and no program recovered from the binary.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// The file name, beside the dumps.
pub const FILE: &str = "build.json";

/// What one extraction was taken from, and when.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildRecord {
    /// The Steam buildid, which is also the directory name.
    pub build_id: String,
    /// The `CodeView` record, when the image carries one.
    pub fingerprint: Option<Fingerprint>,
    /// The revision the data pack states, when it could be read.
    pub revision: Option<Revision>,
    /// When the extraction ran, as RFC 3339 in UTC.
    pub extracted_at: String,
    /// The server executable that was read.
    pub server: String,
    /// The client executable that was read, when one was named.
    pub client: Option<String>,
}

/// The build fingerprint in both renderings that exist for it.
///
/// The two differ, and which one is right depends on who is asking. Recording
/// only one of them is how a lookup silently searches for a build that is not
/// there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fingerprint {
    /// The sixteen signature bytes in the order the `CodeView` record stores
    /// them. A signature row matches on this one.
    pub guid_file_order: String,
    /// The same bytes as Microsoft renders a PDB GUID, with the first three
    /// fields byte swapped. A symbol server takes this one; strip the hyphens
    /// and upper-case it to build a `symsrv` path.
    pub guid_pdb: String,
    /// How many times the PDB was rebuilt for this signature.
    pub age: u32,
}

/// The revision the data pack beside the executable states.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Revision {
    /// The Subversion revision number of the game content tree.
    pub number: u32,
    /// The caret path naming the branch the content came from.
    pub branch: String,
    /// The timestamp the pack was built, as the header writes it.
    pub stamp: String,
}

impl BuildRecord {
    /// Write the record into an extraction directory.
    ///
    /// # Errors
    ///
    /// Returns an error when the record cannot be rendered or written.
    pub fn write(&self, dir: &Path) -> Result<usize> {
        let path = dir.join(FILE);
        let mut body =
            serde_json::to_string_pretty(self).with_context(|| format!("rendering {FILE}"))?;
        body.push('\n');
        std::fs::write(&path, &body).with_context(|| format!("writing {}", path.display()))?;
        Ok(body.len())
    }

    /// Read the record an extraction directory holds, when it holds one.
    ///
    /// An extraction taken before this file existed has none, so a missing file
    /// is not an error. A file that is there and will not parse is.
    ///
    /// # Errors
    ///
    /// Returns an error when the file exists and does not parse.
    pub fn read(dir: &Path) -> Result<Option<Self>> {
        let path = dir.join(FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
        };
        let record =
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        Ok(Some(record))
    }

    /// One line naming the build, for a command that prints a heading.
    pub fn summary(&self) -> String {
        let revision = self.revision.as_ref().map_or_else(
            || "revision unknown".to_string(),
            |r| format!("r{}", r.number),
        );
        format!(
            "{} ({revision}, extracted {})",
            self.build_id, self.extracted_at
        )
    }
}

/// The current time as RFC 3339 in UTC.
///
/// # Errors
///
/// Returns an error when the timestamp cannot be formatted.
pub fn now() -> Result<String> {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .context("formatting the extraction time")
}

#[cfg(test)]
mod tests {
    use super::{BuildRecord, FILE, Fingerprint, Revision, now};
    use crate::testutil::TestDir;

    fn record() -> BuildRecord {
        BuildRecord {
            build_id: "23178631".to_string(),
            fingerprint: Some(Fingerprint {
                guid_file_order: "ae33b44863803d4eb31f31d4d84ae242".to_string(),
                guid_pdb: "48b433ae-8063-4e3d-b31f-31d4d84ae242".to_string(),
                age: 293,
            }),
            revision: Some(Revision {
                number: 1_024_233,
                branch: "^/game38/branches/ea_update_08".to_string(),
                stamp: "2026-05-11T10:44:54.050433Z".to_string(),
            }),
            extracted_at: "2026-09-17T21:00:00Z".to_string(),
            server: r"Z:\cache\server\23178631\enshrouded_server.exe".to_string(),
            client: None,
        }
    }

    #[test]
    fn a_record_survives_a_round_trip_through_the_file() {
        let dir = TestDir::new("metadata");
        let written = record();
        written.write(dir.path()).expect("the record writes");
        let read = BuildRecord::read(dir.path())
            .expect("the record parses")
            .expect("the record is there");
        assert_eq!(read.build_id, written.build_id);
        assert_eq!(read.fingerprint, written.fingerprint);
        assert_eq!(read.revision, written.revision);
        assert_eq!(read.extracted_at, written.extracted_at);
        assert_eq!(read.client, None);
    }

    /// The two renderings differ, so a reader has to be able to tell which is
    /// which from the field name alone.
    #[test]
    fn both_fingerprint_renderings_are_named_and_distinct() {
        let dir = TestDir::new("metadata-names");
        record().write(dir.path()).expect("the record writes");
        let body = std::fs::read_to_string(dir.path().join(FILE)).expect("the file is there");
        assert!(body.contains("\"guidFileOrder\""), "{body}");
        assert!(body.contains("\"guidPdb\""), "{body}");
        assert!(body.contains("ae33b44863803d4eb31f31d4d84ae242"), "{body}");
        assert!(
            body.contains("48b433ae-8063-4e3d-b31f-31d4d84ae242"),
            "{body}"
        );
    }

    #[test]
    fn a_directory_with_no_record_reads_as_none() {
        let dir = TestDir::new("metadata-absent");
        assert!(
            BuildRecord::read(dir.path())
                .expect("absence is not an error")
                .is_none()
        );
    }

    #[test]
    fn a_record_that_does_not_parse_is_an_error() {
        let dir = TestDir::new("metadata-broken");
        dir.write(FILE, b"{ not json");
        let err = BuildRecord::read(dir.path()).expect_err("a broken record is an error");
        assert!(format!("{err:#}").contains(FILE), "{err:#}");
    }

    #[test]
    fn the_extraction_time_is_rfc_3339_in_utc() {
        let stamp = now().expect("the clock formats");
        assert!(stamp.ends_with('Z'), "expected UTC, got {stamp}");
        assert_eq!(
            stamp.as_bytes()[4],
            b'-',
            "expected a date first, got {stamp}"
        );
        assert_eq!(
            stamp.as_bytes()[10],
            b'T',
            "expected a date and a time, got {stamp}"
        );
    }
}
