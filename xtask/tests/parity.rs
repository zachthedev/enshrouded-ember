//! The extractor reproduces a known dump byte for byte.
//!
//! The dumps this compares against were produced from a real server build by
//! the Python extractors this port replaces. They are not in the repository and
//! never will be: nothing recovered from a Keen binary is committed. The test
//! reads them from a directory the caller names and skips when it is unset.
//!
//! Run it with the two variables set:
//!
//! ```text
//! EMBER_SERVER_EXE=<path>\enshrouded_server.exe
//! EMBER_SCHEMA=<path>\.cache\schema\<buildid>
//! cargo test -p xtask -- --ignored
//! ```
//!
//! `EMBER_CLIENT_EXE` adds the client dump to the comparison when it is set.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The variable naming the server executable to read.
const SERVER_VAR: &str = "EMBER_SERVER_EXE";

/// The variable naming the client executable, which is optional.
const CLIENT_VAR: &str = "EMBER_CLIENT_EXE";

/// The variable naming the directory holding the dumps to match.
const SCHEMA_VAR: &str = "EMBER_SCHEMA";

/// The dumps a server build alone produces.
const SERVER_OUTPUTS: [&str; 6] = [
    "srv.schema.txt",
    "srv.proto.txt",
    "ui-events.txt",
    "descriptors.tsv",
    "strings.tsv",
    "programs.tsv",
];

/// The dump only a client build produces.
const CLIENT_OUTPUT: &str = "cli.schema.txt";

/// The record beside the dumps. It names the build and the moment the
/// extraction ran, so it differs from run to run by construction and is checked
/// for shape rather than compared byte for byte.
const RECORD: &str = "build.json";

#[test]
#[ignore = "needs a fetched server build and its dumps, named by environment"]
fn the_extractor_reproduces_the_recorded_dumps() {
    let (Some(server), Some(schema)) = (var(SERVER_VAR), var(SCHEMA_VAR)) else {
        eprintln!("skipped: set {SERVER_VAR} and {SCHEMA_VAR} to run this");
        return;
    };
    assert!(
        server.is_file(),
        "{SERVER_VAR} names {}, which is not a file",
        server.display()
    );
    assert!(
        schema.is_dir(),
        "{SCHEMA_VAR} names {}, which is not a directory",
        schema.display()
    );

    let root = sandbox("parity");
    let client = var(CLIENT_VAR);
    let mut args: Vec<String> = vec![
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "schema".to_string(),
        "extract".to_string(),
        "--build".to_string(),
        "parity".to_string(),
        "--server".to_string(),
        server.to_string_lossy().into_owned(),
        "--force".to_string(),
    ];
    if let Some(client) = &client {
        args.push("--client".to_string());
        args.push(client.to_string_lossy().into_owned());
    }

    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(&args)
        .output()
        .expect("the extractor runs");
    assert!(
        output.status.success(),
        "schema extract failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let produced = root.join(".cache").join("schema").join("parity");
    let mut wanted: Vec<&str> = SERVER_OUTPUTS.to_vec();
    if client.is_some() {
        wanted.push(CLIENT_OUTPUT);
    }
    let mut differences: Vec<String> = Vec::new();
    check_record(&produced, "parity", &server);
    for name in wanted {
        let mine = std::fs::read(produced.join(name))
            .unwrap_or_else(|err| panic!("reading the extracted {name}: {err}"));
        let theirs = std::fs::read(schema.join(name))
            .unwrap_or_else(|err| panic!("reading the recorded {name}: {err}"));
        if mine != theirs {
            differences.push(describe(name, &mine, &theirs));
        }
    }
    let _ = std::fs::remove_dir_all(&root);
    assert!(
        differences.is_empty(),
        "the port does not reproduce the recorded dumps:\n{}",
        differences.join("\n")
    );
}

/// The record is written, names this build, and carries both renderings of the
/// fingerprint.
///
/// It is deliberately outside the byte comparison: it holds the extraction
/// time, so two runs of the same extractor produce two different files.
fn check_record(produced: &Path, build_id: &str, server: &Path) {
    let path = produced.join(RECORD);
    let body = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("reading {}: {err}", path.display()));
    for wanted in [
        &format!("\"buildId\": \"{build_id}\""),
        "\"guidFileOrder\"",
        "\"guidPdb\"",
        "\"extractedAt\"",
        "\"revision\"",
    ] {
        assert!(
            body.contains(wanted),
            "{RECORD} is missing {wanted}: {body}"
        );
    }
    let server_name = server
        .file_name()
        .and_then(|name| name.to_str())
        .expect("the server executable has a name");
    assert!(
        body.contains(server_name),
        "{RECORD} must name the image it read: {body}"
    );
    assert!(
        !body.contains("\"guidFileOrder\": \"48b433ae"),
        "the file-order rendering must not carry the PDB byte order: {body}"
    );
}

/// Name the first line that differs, so a failure points at bytes.
fn describe(name: &str, mine: &[u8], theirs: &[u8]) -> String {
    let mine_lines: Vec<&[u8]> = mine.split(|byte| *byte == b'\n').collect();
    let theirs_lines: Vec<&[u8]> = theirs.split(|byte| *byte == b'\n').collect();
    let first = mine_lines
        .iter()
        .zip(&theirs_lines)
        .position(|(a, b)| a != b);
    match first {
        Some(index) => format!(
            "  {name}: line {} differs\n    extracted {}\n    recorded  {}",
            index + 1,
            String::from_utf8_lossy(mine_lines[index]),
            String::from_utf8_lossy(theirs_lines[index])
        ),
        None => format!(
            "  {name}: {} lines extracted against {} recorded, {} bytes against {}",
            mine_lines.len(),
            theirs_lines.len(),
            mine.len(),
            theirs.len()
        ),
    }
}

/// Read an environment variable as a path, treating empty as unset.
fn var(name: &str) -> Option<PathBuf> {
    let value = std::env::var_os(name)?;
    (!value.is_empty()).then(|| PathBuf::from(value))
}

/// A directory of this test's own, under the process temp directory.
fn sandbox(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("ember-xtask-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("creating the sandbox");
    path
}
