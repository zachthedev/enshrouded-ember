//! The files that pin every tool the gate runs, and the rules they meet.
//!
//! `mise.toml` holds one version per tool and `mise.lock` holds a checksum per
//! platform for each of them. These rules run before the gate runs any tool,
//! because a lockfile entry carrying a url and no checksum installs whatever
//! that url serves: mise's locked mode requires an entry rather than a
//! checksum, and it does not write a missing checksum back. A rule that runs
//! after the install reports a finding about a binary that already executed.

use std::collections::BTreeSet;

/// The file pinning a version for every tool mise installs.
pub const PINS: &str = "mise.toml";

/// The file holding a checksum per platform for every tool [`PINS`] pins.
pub const LOCK: &str = "mise.lock";

/// The workflow whose matrix says which platforms an install has to cover.
pub const WORKFLOW: &str = ".github/workflows/ci.yml";

/// The command that rewrites [`LOCK`] after an edit to [`PINS`].
pub const RELOCK: &str = "mise lock --platform linux-x64,windows-x64";

/// The mise platform each runner label the gate job's matrix names installs
/// for.
///
/// A label with no entry here has no platform to hold the lockfile to, which
/// is a refusal rather than a skip: a leg added without a relock installs
/// whatever its platform's url serves.
pub const RUNNER_PLATFORMS: &[(&str, &str)] = &[
    ("ubuntu-latest", "linux-x64"),
    ("windows-latest", "windows-x64"),
];

/// One platform entry the lockfile records: the tool, the platform, the version
/// and the checksum, with `None` where the entry carries none.
pub type LockedPlatform = (String, String, String, Option<String>);

/// The binary each backend coordinate in [`PINS`] installs.
///
/// A bare registry name is the binary's own name, so nothing here is needed for
/// one. A coordinate names an owner and a repository, and the binary inside is
/// neither, so it is written here. A coordinate added to [`PINS`] without an
/// entry here is refused, and so is an entry here naming a coordinate [`PINS`]
/// no longer holds.
pub const COORDINATE_BINARIES: &[(&str, &str)] = &[
    ("github:bnjbvr/cargo-machete", "cargo-machete"),
    ("github:nextest-rs/nextest", "cargo-nextest"),
    ("github:rustsec/rustsec", "cargo-audit"),
];

/// The version [`PINS`] holds for the tool spelled `name`, or `None` when its
/// `[tools]` table has no such entry.
///
/// The value is the version itself, or a table carrying it under `version`,
/// which is the shape an entry with backend options takes. The key is matched
/// whole, so a backend coordinate is never read as the tool at the end of it.
/// Text that is not TOML reads as no version, which the gate reports the same
/// way as a missing entry: nothing says which release.
#[must_use]
pub fn pinned_version(text: &str, name: &str) -> Option<String> {
    let document: toml::Value = toml::from_str(text).ok()?;
    match document.get("tools")?.get(name)? {
        toml::Value::String(version) => Some(version.clone()),
        entry => entry.get("version")?.as_str().map(str::to_string),
    }
}

/// Every tool [`PINS`] names, as the key it is spelled under and its version.
///
/// # Errors
///
/// Returns the sentence a result row carries when the text is not TOML or
/// holds no `[tools]` table.
pub fn pinned_tools(text: &str) -> Result<Vec<(String, String)>, String> {
    let document: toml::Value =
        toml::from_str(text).map_err(|err| format!("{PINS} is not TOML: {err}"))?;
    let tools = document
        .get("tools")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| format!("{PINS} holds no tools table"))?;
    tools
        .keys()
        .map(|name| {
            let version = pinned_version(text, name)
                .ok_or_else(|| format!("{PINS} pins no version for {name}"))?;
            Ok((name.clone(), version))
        })
        .collect()
}

/// Whether [`PINS`] turns mise's locked mode on.
///
/// Locked mode is what makes an install take an artifact [`LOCK`] records. With
/// it off, `mise install` accepts a tool the lockfile does not name, and
/// `mise which` answers for a tool this repository pins nowhere out of a
/// developer's global configuration. The action that installs on a runner adds
/// `--locked` itself when it sees a lockfile, so continuous integration keeps
/// most of the guarantee while a developer's machine loses all of it.
#[must_use]
pub fn locked(text: &str) -> bool {
    toml::from_str::<toml::Value>(text)
        .ok()
        .and_then(|document| document.get("settings")?.get("locked")?.as_bool())
        .unwrap_or(false)
}

/// The binary the gate runs for the [`PINS`] key `name`.
///
/// # Errors
///
/// Returns the sentence a result row carries when `name` is a coordinate
/// [`COORDINATE_BINARIES`] does not name.
pub fn binary_for(name: &str) -> Result<String, String> {
    if !name.contains(':') {
        return Ok(name.to_string());
    }
    COORDINATE_BINARIES
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, binary)| (*binary).to_string())
        .ok_or_else(|| format!("{name} is a coordinate no entry names a binary for"))
}

/// Every binary the gate runs, derived from the keys [`PINS`] holds.
///
/// # Errors
///
/// Returns the first sentence [`binary_for`] or [`pinned_tools`] produces.
pub fn pinned_binaries(text: &str) -> Result<BTreeSet<String>, String> {
    pinned_tools(text)?
        .into_iter()
        .map(|(name, _)| binary_for(&name))
        .collect()
}

/// Every platform entry [`LOCK`] records, as the tool, the platform, the
/// version and the checksum the entry carries.
///
/// mise generates the file and owns its layout, so this parses it and reads by
/// key rather than by position. A tool's value is an array of tables, one per
/// locked version, and a platform is a key spelled `platforms.<name>`. An entry
/// with no `checksum` yields `None`, which is the shape a backend with no
/// artifact to hash writes and the shape an artifact with no recorded digest
/// takes.
///
/// # Errors
///
/// Returns the sentence a result row carries when the text is not TOML or does
/// not hold the shape mise writes.
pub fn locked_platforms(text: &str) -> Result<Vec<LockedPlatform>, String> {
    let document: toml::Value =
        toml::from_str(text).map_err(|err| format!("{LOCK} is not TOML: {err}"))?;
    let tools = document
        .get("tools")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| format!("{LOCK} holds no tools table"))?;
    let mut found = Vec::new();
    for (name, entries) in tools {
        let entries = entries
            .as_array()
            .ok_or_else(|| format!("{LOCK} holds {name} as something other than entries"))?;
        for entry in entries {
            let table = entry.as_table().ok_or_else(|| {
                format!("{LOCK} holds a {name} entry as something other than a table")
            })?;
            let version = table
                .get("version")
                .and_then(toml::Value::as_str)
                .unwrap_or_default()
                .to_string();
            for (key, value) in table {
                let Some(platform) = key.strip_prefix("platforms.") else {
                    continue;
                };
                found.push((
                    name.clone(),
                    platform.to_string(),
                    version.clone(),
                    value
                        .get("checksum")
                        .and_then(toml::Value::as_str)
                        .map(str::to_string),
                ));
            }
        }
    }
    Ok(found)
}

/// The platforms the gate job installs on, read from its own matrix.
///
/// The matrix is read as text rather than as YAML, so this module needs no
/// parser of its own and the gate can run it before anything else.
///
/// # Errors
///
/// Returns the sentence a result row carries when the job declares no matrix or
/// names a runner [`RUNNER_PLATFORMS`] does not cover.
pub fn matrix_platforms(text: &str) -> Result<Vec<String>, String> {
    let labels = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("os: ["))
        .ok_or_else(|| format!("{WORKFLOW} declares no os matrix"))?;
    let labels: Vec<&str> = labels
        .trim_end_matches(']')
        .split(',')
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .collect();
    if labels.is_empty() {
        return Err(format!("{WORKFLOW} declares an empty os matrix"));
    }
    labels
        .iter()
        .map(|label| {
            RUNNER_PLATFORMS
                .iter()
                .find(|(runner, _)| runner == label)
                .map(|(_, platform)| (*platform).to_string())
                .ok_or_else(|| {
                    format!("{label} has no mise platform, so its lockfile entry goes unchecked")
                })
        })
        .collect()
}

/// Every way the pin files fall short, as one sentence each.
///
/// The three texts are passed in rather than read here, so the rules run the
/// same way against the repository and against a case written in a test.
#[must_use]
pub fn problems(pins: &str, lock: &str, workflow: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();

    if !locked(pins) {
        found.push(format!(
            "{PINS} does not set locked = true, so an install takes whatever a registry serves"
        ));
    }

    let tools = match pinned_tools(pins) {
        Ok(tools) => tools,
        Err(problem) => return vec![problem],
    };
    if tools.is_empty() {
        return vec![format!("{PINS} pins nothing")];
    }
    for (name, version) in &tools {
        let exact = version.split('.').count() == 3
            && version
                .split('.')
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
        if !exact {
            found.push(format!(
                "{PINS} pins {name} at {version}, which is not one exact release"
            ));
        }
    }
    if let Err(problem) = pinned_binaries(pins) {
        found.push(problem);
    }

    let platforms = match matrix_platforms(workflow) {
        Ok(platforms) => platforms,
        Err(problem) => {
            found.push(problem);
            return found;
        }
    };
    let locked_entries = match locked_platforms(lock) {
        Ok(entries) => entries,
        Err(problem) => {
            found.push(problem);
            return found;
        }
    };

    for (name, version) in &tools {
        for platform in &platforms {
            match locked_entries
                .iter()
                .find(|(tool, at, _, _)| tool == name && at == platform)
            {
                None => found.push(format!("{name} records no {platform} entry in {LOCK}")),
                Some((_, _, _, None)) => {
                    found.push(format!(
                        "{name} on {platform} carries no checksum in {LOCK}"
                    ));
                }
                Some((_, _, locked_version, Some(checksum))) => {
                    if !checksum.starts_with("sha256:") || checksum.len() <= "sha256:".len() {
                        found.push(format!("{name} on {platform} carries {checksum}"));
                    }
                    if locked_version != version {
                        found.push(format!(
                            "{PINS} pins {name} {version} and {LOCK} records {locked_version}"
                        ));
                    }
                }
            }
        }
    }

    let pinned: BTreeSet<&String> = tools.iter().map(|(name, _)| name).collect();
    for name in locked_entries
        .iter()
        .map(|(tool, _, _, _)| tool)
        .collect::<BTreeSet<&String>>()
    {
        if !pinned.contains(name) {
            found.push(format!("{LOCK} locks {name}, which {PINS} no longer pins"));
        }
    }

    found.sort_unstable();
    found.dedup();
    found
}
