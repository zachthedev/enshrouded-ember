//! The files that pin every tool the gate runs, and the rules they meet.
//!
//! `mise.toml` holds one version per tool and `mise.lock` holds a checksum, a
//! url and a backend per platform for each of them. These rules run before the
//! gate runs any tool, because a lockfile entry decides what an install
//! downloads: mise's locked mode fetches the url the entry records rather than
//! asking the backend, and it compares the checksum the entry records. A rule
//! that runs after the install reports a finding about a binary that already
//! executed.
//!
//! A checksum rule alone holds the lockfile to itself. The url, the backend and
//! the checksum all sit in the generated file, so an edit that moves all three
//! together leaves every one of them agreeing. What the url and backend rules
//! add is a second document to disagree with: the owner and the repository each
//! artifact belongs to live in [`TOOLS`], in source, so an artifact moving to
//! another host or another account takes an edit a reviewer reads.

use std::collections::BTreeSet;

/// The file pinning a version for every tool mise installs.
pub const PINS: &str = "mise.toml";

/// The file holding a checksum, a url and a backend per platform for every tool
/// [`PINS`] pins.
pub const LOCK: &str = "mise.lock";

/// The workflow whose matrix says which platforms an install has to cover.
pub const WORKFLOW: &str = ".github/workflows/ci.yml";

/// The [`WORKFLOW`] job that installs the pinned tools and runs the gate.
///
/// The ordering rule names this job rather than searching for one that looks
/// like it installs. A search skips a job whose installer it does not
/// recognize, and a skipped job is indistinguishable from a job with nothing to
/// check.
pub const GATE_JOB: &str = "gate";

/// The actions [`GATE_JOB`] may run before the pin rules.
///
/// Anything else a step `uses` is treated as an install, so an action this list
/// does not name fails the ordering rule rather than passing unread. A composite
/// action is free to install whatever it likes, and its name says nothing about
/// whether it does.
pub const PRELUDE_ACTIONS: &[&str] = &[
    "actions/checkout",
    "Swatinem/rust-cache",
    "oven-sh/setup-bun",
];

/// The mise invocations that neither fetch nor write, as the words that open
/// them.
///
/// Every other invocation counts as an install, for the same reason
/// [`PRELUDE_ACTIONS`] is an allow list. mise fetches on demand for several
/// subcommands and `install` answers to an alias, so a list of the ones that
/// fetch would only ever be the ones somebody has already thought of.
///
/// `settings` and `config` are named with their second word because each has a
/// `set` that rewrites [`PINS`]. `mise config set` reaches `tool_config.locked`,
/// which is the one setting no environment variable reaches, so a one-word
/// entry for either would bless the command that undoes the rule those tables
/// exist for. A step rewriting [`PINS`] after these rules read it leaves the
/// install running against a document nothing checked.
pub const READING_MISE_COMMANDS: &[&[&str]] = &[
    &["which"],
    &["doctor"],
    &["version"],
    &["--version"],
    &["-v"],
    &["settings", "get"],
    &["settings", "ls"],
    &["config", "get"],
    &["config", "ls"],
];

/// The urls that fetch mise itself.
///
/// A step fetching mise is installing, so both documented bootstrap urls are
/// named. One of them alone leaves the other reading as an ordinary command.
pub const MISE_BOOTSTRAP_URLS: &[&str] = &["mise.run", "mise.jdx.dev/install.sh"];

/// The command that rewrites [`LOCK`] after an edit to [`PINS`].
pub const RELOCK: &str = "mise lock --platform linux-x64,windows-x64";

/// The host every release artifact [`LOCK`] records is served from.
pub const RELEASE_HOST: &str = "github.com";

/// The host every release api reference [`LOCK`] records is served from.
pub const API_HOST: &str = "api.github.com";

/// The provenance [`LOCK`] records for a release carrying an attestation.
pub const ATTESTED: &str = "github-attestations";

/// The digits a sha256 digest is written with, after its `sha256:` prefix.
const SHA256_DIGITS: usize = 64;

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

/// The backend mise installs a tool through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// mise's registry, which routes a bare tool name to a packaged release.
    Aqua,
    /// A repository's own releases, named in [`PINS`] as the key itself.
    Github,
}

impl Backend {
    /// The word [`LOCK`] spells this backend with, ahead of the coordinate.
    #[must_use]
    pub const fn prefix(self) -> &'static str {
        match self {
            Self::Aqua => "aqua",
            Self::Github => "github",
        }
    }
}

/// A tool [`PINS`] names, and the GitHub release its artifacts come from.
pub struct Tool {
    /// The key [`PINS`] spells the tool under.
    pub key: &'static str,
    /// The binary the gate runs once mise installs it.
    pub binary: &'static str,
    /// The backend mise installs it through.
    pub backend: Backend,
    /// The account owning the repository the release belongs to.
    pub owner: &'static str,
    /// The repository the release belongs to.
    pub repository: &'static str,
    /// What the release tag carries ahead of the version.
    ///
    /// The tag is this and the version and nothing else, which is what lets the
    /// rules compare it whole. A registry key carries no prefix of its own, so
    /// for an aqua tool this is the only record of whether its tags take a `v`.
    pub tag_prefix: &'static str,
    /// The provenance [`LOCK`] has to record for this tool, or `None` for a
    /// release carrying no attestation.
    ///
    /// `locked_verify_provenance` re-verifies an attestation against the
    /// artifact, and the per-platform `provenance` line is what marks a tool as
    /// needing one. That line lives in the generated file, so the expected value
    /// is held here and a line deleted there contradicts this table.
    pub provenance: Option<&'static str>,
}

impl Tool {
    /// The coordinate [`LOCK`] has to record as this tool's backend.
    ///
    /// Both backends spell a coordinate as the backend word, a colon, the owner
    /// and the repository. A bare [`PINS`] key names neither the owner nor the
    /// repository, so for an aqua tool this is the only place they are written
    /// down outside the generated file.
    #[must_use]
    pub fn coordinate(&self) -> String {
        format!(
            "{}:{}/{}",
            self.backend.prefix(),
            self.owner,
            self.repository
        )
    }

    /// The path prefix every artifact url for this tool sits under.
    #[must_use]
    pub fn release_prefix(&self) -> String {
        format!("/{}/{}/releases/download/", self.owner, self.repository)
    }

    /// The path prefix every release api reference for this tool sits under.
    ///
    /// The api addresses a release by asset number rather than by tag, so this
    /// prefix carries no version and nothing downstream looks for one in it.
    #[must_use]
    pub fn api_prefix(&self) -> String {
        format!("/repos/{}/{}/releases/", self.owner, self.repository)
    }

    /// The release tag every artifact url for this tool sits under, for
    /// `version`.
    ///
    /// A tag is the prefix and the version joined, so this is compared whole
    /// rather than searched for. A search would accept a tag that merely holds
    /// the version, which is what a prerelease tag and an older tag naming the
    /// version in the filename both do.
    #[must_use]
    pub fn tag(&self, version: &str) -> String {
        format!("{}{version}", self.tag_prefix)
    }
}

/// Every tool the gate runs, with the release each one's artifacts come from.
///
/// The owner and the repository are held here rather than read from [`LOCK`],
/// and that is what gives the rules a second document to hold the first to.
/// [`LOCK`] is generated and gets replaced wholesale, so a rewrite of it
/// carries whatever owner and host it likes. A bare registry key in [`PINS`]
/// names no owner at all, which leaves the generated file as the only place an
/// aqua tool's account appears. Reading the expected account from here instead
/// means an artifact that moves to another account takes an edit in this file,
/// in the same diff as the lockfile it explains.
///
/// A [`PINS`] key with no entry here is refused, and so is an entry here naming
/// a key [`PINS`] no longer holds.
pub const TOOLS: &[Tool] = &[
    Tool {
        key: "actionlint",
        binary: "actionlint",
        backend: Backend::Aqua,
        owner: "rhysd",
        repository: "actionlint",
        tag_prefix: "v",
        provenance: Some(ATTESTED),
    },
    Tool {
        key: "cargo-deny",
        binary: "cargo-deny",
        backend: Backend::Aqua,
        owner: "EmbarkStudios",
        repository: "cargo-deny",
        tag_prefix: "",
        provenance: None,
    },
    Tool {
        key: "github:bnjbvr/cargo-machete",
        binary: "cargo-machete",
        backend: Backend::Github,
        owner: "bnjbvr",
        repository: "cargo-machete",
        tag_prefix: "v",
        provenance: None,
    },
    Tool {
        key: "github:nextest-rs/nextest",
        binary: "cargo-nextest",
        backend: Backend::Github,
        owner: "nextest-rs",
        repository: "nextest",
        tag_prefix: "cargo-nextest-",
        provenance: Some(ATTESTED),
    },
    Tool {
        key: "github:rustsec/rustsec",
        binary: "cargo-audit",
        backend: Backend::Github,
        owner: "rustsec",
        repository: "rustsec",
        tag_prefix: "cargo-audit/v",
        provenance: None,
    },
    Tool {
        key: "shellcheck",
        binary: "shellcheck",
        backend: Backend::Aqua,
        owner: "koalaman",
        repository: "shellcheck",
        tag_prefix: "v",
        provenance: None,
    },
    Tool {
        key: "taplo",
        binary: "taplo",
        backend: Backend::Aqua,
        owner: "tamasfe",
        repository: "taplo",
        tag_prefix: "",
        provenance: None,
    },
    Tool {
        key: "zizmor",
        binary: "zizmor",
        backend: Backend::Aqua,
        owner: "zizmorcore",
        repository: "zizmor",
        tag_prefix: "v",
        provenance: Some(ATTESTED),
    },
];

/// The tool [`TOOLS`] holds for the [`PINS`] key `name`, or `None` for a key it
/// does not name.
#[must_use]
pub fn tool_for(name: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|tool| tool.key == name)
}

/// One platform entry [`LOCK`] records.
pub struct LockedEntry {
    /// The [`PINS`] key the entry sits under.
    pub tool: String,
    /// The mise platform the entry installs for.
    pub platform: String,
    /// The release the entry records.
    pub version: String,
    /// The backend coordinate the entry records, empty where it records none.
    pub backend: String,
    /// The digest the entry records, `None` where it records none.
    pub checksum: Option<String>,
    /// The artifact url the entry records, `None` where it records none.
    pub url: Option<String>,
    /// The release api reference the entry records, `None` where it records
    /// none.
    pub url_api: Option<String>,
    /// The provenance the entry records, `None` where it records none.
    pub provenance: Option<String>,
}

/// What [`LOCK`] records for one tool, counted where the file is parsed.
///
/// A tool's value is an array of tables, so a second table is legal TOML.
/// Counting here rather than over the platform rows is what makes an entry
/// carrying no platform block visible: such an entry produces no row, and a
/// count taken from rows never sees it.
pub struct LockedTool {
    /// The [`PINS`] key the entries sit under.
    pub tool: String,
    /// How many array entries the tool records.
    pub entries: usize,
    /// How many of those record no platform block at all.
    pub platformless: usize,
}

/// An absolute `https` url, split into the parts a rule reads.
pub struct Absolute<'a> {
    /// The host the request lands on.
    pub host: &'a str,
    /// The path, whose segments are literal because [`absolute`] refuses any
    /// url whose segments are not.
    pub path: &'a str,
}

/// `text` as an absolute `https` url, or `None` when it is anything else.
///
/// This refuses rather than normalizes, because every url mise writes is a
/// plain `https` release url and anything else is already wrong. Userinfo, a
/// port, a percent escape in the authority and a backslash each move where a
/// request lands while leaving the text looking familiar, and a `.` or `..`
/// segment moves where a path resolves to. A query or a fragment would let text
/// after the path carry the version a caller looks for. Each is refused here,
/// so a caller compares a host whole and reads a path whose segments are
/// literal.
///
/// A percent escape is refused in the path as well as the authority, because a
/// segment check reads literal text and `%2e%2e` is a traversal this rule would
/// otherwise walk past. Every byte below `0x21` goes too: a url parser drops
/// tab, newline and carriage return while parsing, so a path carrying one is a
/// string that differs from the url a client fetches.
///
/// Refusing is what separates this from a text match. `https://github.com.example/x`
/// holds every substring a search for the host would find, and its host is
/// `github.com.example`.
#[must_use]
pub fn absolute(text: &str) -> Option<Absolute<'_>> {
    let rest = text.strip_prefix("https://")?;
    if rest.contains('\\') {
        return None;
    }
    let at = rest.find('/')?;
    let (authority, path) = rest.split_at(at);
    if authority.is_empty()
        || authority.contains('@')
        || authority.contains(':')
        || authority.contains('%')
    {
        return None;
    }
    if path.contains('?') || path.contains('#') || path.contains('%') {
        return None;
    }
    // Printable ASCII only. A space, a tab, a newline or a DEL is dropped or
    // rewritten by a url parser, so the text a rule read would differ from the
    // url a client fetches. Anything above that range renders as something
    // other than what it is: a zero-width space hides inside a name and a
    // right-to-left override reverses one in a reviewer's diff.
    if path.bytes().any(|byte| !(0x21..=0x7e).contains(&byte)) {
        return None;
    }
    if path
        .split('/')
        .any(|segment| segment == "." || segment == "..")
    {
        return None;
    }
    Some(Absolute {
        host: authority,
        path,
    })
}

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

/// Whether [`PINS`] sets `locked` in its `[settings]` table.
///
/// This is the locked mode a developer's own environment answers for.
/// `MISE_LOCKED=false` turns it off, and so does a `MISE_LOCKED_SCOPES` that
/// leaves out the project scope, in both cases while `mise settings get locked`
/// still prints what this file holds. [`tool_config_locked`] is the setting
/// neither reaches, and the two are asserted together for that reason.
#[must_use]
pub fn locked(text: &str) -> bool {
    toml::from_str::<toml::Value>(text)
        .ok()
        .and_then(|document| document.get("settings")?.get("locked")?.as_bool())
        .unwrap_or(false)
}

/// Whether [`PINS`] sets `locked` in its `[tool_config]` table.
///
/// This is the locked mode that holds whatever the environment says. mise reads
/// it from the file alone, so the value here is the value that applies and no
/// effective-value lookup can differ from it. It is also the only one of the
/// two that mise refuses an unlocked install under when `MISE_LOCKED_SCOPES`
/// drops the project scope.
///
/// `mise settings get` answers `Unknown setting` for it, so this file is where
/// it is read from.
#[must_use]
pub fn tool_config_locked(text: &str) -> bool {
    toml::from_str::<toml::Value>(text)
        .ok()
        .and_then(|document| document.get("tool_config")?.get("locked")?.as_bool())
        .unwrap_or(false)
}

/// Whether [`PINS`] sets `locked_verify_provenance` in its `[settings]` table.
///
/// With it off, an install trusts the `provenance` value [`LOCK`] records and
/// checks only that the matching setting is on, so an entry claiming an
/// attestation it does not have installs without one. With it on, the
/// attestation is re-verified against the artifact. A tool whose entry records
/// no provenance installs on its checksum either way.
#[must_use]
pub fn verify_provenance(text: &str) -> bool {
    toml::from_str::<toml::Value>(text)
        .ok()
        .and_then(|document| {
            document
                .get("settings")?
                .get("locked_verify_provenance")?
                .as_bool()
        })
        .unwrap_or(false)
}

/// The environment variables naming which config filenames mise reads.
///
/// `MISE_OVERRIDE_CONFIG_FILENAMES` replaces the filenames mise reads in a
/// directory and `MISE_DEFAULT_CONFIG_FILENAME` names the one it writes. Each
/// takes a colon-separated list, and the last name in that list decides, so a
/// value naming [`PINS`] beside another file leaves mise reading the other one.
pub const CONFIG_FILENAME_VARIABLES: &[&str] = &[
    "MISE_OVERRIDE_CONFIG_FILENAMES",
    "MISE_DEFAULT_CONFIG_FILENAME",
];

/// The environment variables that add a config file whatever their value.
///
/// `MISE_ENV` makes mise read `mise.<value>.toml` alongside [`PINS`], and the
/// added file wins. No value of it leaves [`PINS`] as the only document, so any
/// value at all is refused.
///
/// # What the two lists cover
///
/// These are the mechanisms found, not a proof that mise has no others, and a
/// later release can add one. Ten variables were measured against the mise
/// release [`WORKFLOW`] installs, each pointed at a file turning both locked
/// settings off, and only two moved mise off [`PINS`]. Both are covered:
///
/// | variable                         | locked reads | covered |
/// | -------------------------------- | ------------ | ------- |
/// | `MISE_ENV`                       | false        | yes     |
/// | `MISE_OVERRIDE_CONFIG_FILENAMES` | false        | yes     |
/// | `MISE_DEFAULT_CONFIG_FILENAME`   | true         | yes     |
/// | `MISE_CONFIG_FILE`               | true         | no      |
/// | `MISE_GLOBAL_CONFIG_FILE`        | true         | no      |
/// | `MISE_CONFIG_DIR`                | true         | no      |
/// | `MISE_PROJECT_ROOT`              | true         | no      |
/// | `MISE_PROJECT_DIR`               | true         | no      |
/// | `MISE_IGNORED_CONFIG_PATHS`      | true         | no      |
/// | `MISE_GLOBAL_CONFIG_ROOT`        | true         | no      |
///
/// The seven that read `true` left mise on [`PINS`] and need no entry. They are
/// written down so that whoever widens either list knows what was already ruled
/// out, and on which release.
pub const CONFIG_ADDING_VARIABLES: &[&str] = &["MISE_ENV"];

/// Every way the environment points mise at a file these rules do not read.
///
/// `read` answers with the value of the named variable, or `None` where it is
/// unset. An unset variable is the ordinary case and reports nothing.
///
/// A filename list is held to naming [`PINS`] and nothing else. Membership is
/// the wrong question, because mise takes the last name in the list, so a value
/// naming [`PINS`] first and another file second reads the other file.
///
/// This reads the environment rather than asking mise, so it costs no process
/// and the rules stay runnable before mise exists on a machine.
#[must_use]
pub fn environment_problems(read: impl Fn(&str) -> Option<String>) -> Vec<String> {
    let mut found = Vec::new();
    for name in CONFIG_FILENAME_VARIABLES {
        let Some(value) = read(name) else {
            continue;
        };
        if !value.split(':').all(|entry| entry.trim() == PINS) {
            found.push(format!(
                "{name} is set to {value}, and mise reads the last name in that list"
            ));
        }
    }
    for name in CONFIG_ADDING_VARIABLES {
        let Some(value) = read(name) else {
            continue;
        };
        found.push(format!(
            "{name} is set to {value}, so mise reads a file beside {PINS} and prefers it"
        ));
    }
    found
}

/// The binary the gate runs for the [`PINS`] key `name`.
///
/// # Errors
///
/// Returns the sentence a result row carries when `name` is a key [`TOOLS`]
/// does not name.
pub fn binary_for(name: &str) -> Result<String, String> {
    tool_for(name)
        .map(|tool| tool.binary.to_string())
        .ok_or_else(|| format!("{name} is a key no entry names a binary and a release for"))
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

/// Every platform entry [`LOCK`] records.
///
/// mise generates the file and owns its layout, so this parses it and reads by
/// key rather than by position. A tool's value is an array of tables, one per
/// locked version, and a platform is a key spelled `platforms.<name>`. The
/// backend sits on the tool's own table rather than on the platform's, so it is
/// read once and carried onto each platform the tool records.
///
/// # Errors
///
/// Returns the sentence a result row carries when the text is not TOML or does
/// not hold the shape mise writes.
pub fn locked_platforms(text: &str) -> Result<Vec<LockedEntry>, String> {
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
            let text_at = |key: &str| {
                table
                    .get(key)
                    .and_then(toml::Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            let version = text_at("version");
            let backend = text_at("backend");
            for (key, value) in table {
                let Some(platform) = key.strip_prefix("platforms.") else {
                    continue;
                };
                let at = |field: &str| {
                    value
                        .get(field)
                        .and_then(toml::Value::as_str)
                        .map(str::to_string)
                };
                found.push(LockedEntry {
                    tool: name.clone(),
                    platform: platform.to_string(),
                    version: version.clone(),
                    backend: backend.clone(),
                    checksum: at("checksum"),
                    url: at("url"),
                    url_api: at("url_api"),
                    provenance: at("provenance"),
                });
            }
        }
    }
    Ok(found)
}

/// How many entries [`LOCK`] records for each tool, counted while it is parsed.
///
/// An entry carrying no platform block produces no [`LockedEntry`], so a count
/// taken from those rows cannot see it. This counts the array itself.
///
/// # Errors
///
/// Returns the sentence a result row carries when the text is not TOML or does
/// not hold the shape mise writes.
pub fn locked_tools(text: &str) -> Result<Vec<LockedTool>, String> {
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
        let platformless = entries
            .iter()
            .filter(|entry| {
                entry
                    .as_table()
                    .is_none_or(|table| !table.keys().any(|key| key.starts_with("platforms.")))
            })
            .count();
        found.push(LockedTool {
            tool: name.clone(),
            entries: entries.len(),
            platformless,
        });
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

/// Every way one entry's url falls short of the release it claims to serve.
///
/// `label` opens each sentence, `expected` is the host the url has to land on,
/// and `prefix` is the path it has to sit under. `tag`, where given, is the
/// release tag the path has to carry between the prefix and the asset filename.
///
/// The tag is compared whole rather than searched for. A search anywhere in the
/// path accepts a filename that names the release while the tag names an older
/// one, and a search over the tag alone accepts a prerelease tag that merely
/// starts with it. The tag is split off at the last slash, because a tag is free
/// to contain one.
fn url_problems(
    label: &str,
    url: &str,
    expected: &str,
    prefix: &str,
    tag: Option<&str>,
) -> Vec<String> {
    let Some(parsed) = absolute(url) else {
        return vec![format!(
            "{label} records {url}, which is not an absolute https url"
        )];
    };
    let mut found = Vec::new();
    if !parsed.host.eq_ignore_ascii_case(expected) {
        found.push(format!(
            "{label} records a url served by {}, and {expected} serves it",
            parsed.host
        ));
    }
    let Some(rest) = parsed.path.strip_prefix(prefix) else {
        found.push(format!(
            "{label} records a url at {}, and {prefix} holds it",
            parsed.path
        ));
        return found;
    };
    if let Some(tag) = tag {
        match rest.rsplit_once('/') {
            None => found.push(format!(
                "{label} records a url at {}, which names no release tag",
                parsed.path
            )),
            Some((found_tag, asset)) => {
                if found_tag != tag {
                    found.push(format!(
                        "{label} records a url tagged {found_tag}, and {LOCK} records the release {tag}"
                    ));
                }
                if asset.is_empty() {
                    found.push(format!(
                        "{label} records a url at {}, which names no artifact",
                        parsed.path
                    ));
                }
            }
        }
    }
    found
}

/// Every way one platform entry falls short, as one sentence each.
fn entry_problems(tool: &Tool, version: &str, entry: &LockedEntry) -> Vec<String> {
    let label = format!("{} on {}", tool.key, entry.platform);
    let mut found = Vec::new();

    match &entry.checksum {
        None => found.push(format!("{label} carries no checksum in {LOCK}")),
        Some(checksum) => {
            let digits = checksum.strip_prefix("sha256:").unwrap_or_default();
            let sound = digits.len() == SHA256_DIGITS
                && digits.bytes().all(|byte| byte.is_ascii_hexdigit());
            if !sound {
                found.push(format!(
                    "{label} carries {checksum}, and a sha256 digest is {SHA256_DIGITS} hex digits"
                ));
            }
        }
    }
    if entry.version != version {
        found.push(format!(
            "{PINS} pins {} {version} and {LOCK} records {}",
            tool.key, entry.version
        ));
    }

    let coordinate = tool.coordinate();
    if entry.backend != coordinate {
        found.push(format!(
            "{label} installs through {}, and {PINS} pins {coordinate}",
            if entry.backend.is_empty() {
                "no backend"
            } else {
                &entry.backend
            }
        ));
    }

    match &entry.url {
        None => found.push(format!("{label} records no url in {LOCK}")),
        Some(url) => found.extend(url_problems(
            &label,
            url,
            RELEASE_HOST,
            &tool.release_prefix(),
            Some(&tool.tag(version)),
        )),
    }
    // Absence is refused here the same way a missing url is. mise writes this
    // field for every entry, so a lockfile without one has had it taken out,
    // and that is the quiet way to drop the reference an attestation lookup
    // reads.
    match &entry.url_api {
        None => found.push(format!("{label} records no url_api in {LOCK}")),
        Some(url_api) => found.extend(url_problems(
            &format!("{label} api"),
            url_api,
            API_HOST,
            &tool.api_prefix(),
            None,
        )),
    }

    // The provenance line is what makes `locked_verify_provenance` require an
    // attestation for this tool, so the expected value is held in `TOOLS` and a
    // line deleted here contradicts it.
    if entry.provenance.as_deref() != tool.provenance {
        found.push(match (&entry.provenance, tool.provenance) {
            (None, Some(wanted)) => {
                format!("{label} records no provenance, and {wanted} is required for it")
            }
            (Some(carried), None) => {
                format!("{label} records provenance {carried}, and no release of it is attested")
            }
            (carried, wanted) => {
                format!("{label} records provenance {carried:?}, and {wanted:?} is required for it")
            }
        });
    }

    found
}

/// Every way the shape of [`LOCK`]'s tool arrays falls short.
///
/// A tool's value is an array, so a second table beside the sound one is legal
/// TOML and every other rule would read whichever came first. A file holding
/// two answers is refused rather than judged. The count comes from the array
/// itself, because an entry with no platform block produces no row for the
/// other rules to see.
fn array_problems(lock: &str) -> Vec<String> {
    let counted = match locked_tools(lock) {
        Ok(counted) => counted,
        Err(problem) => return vec![problem],
    };
    let mut found = Vec::new();
    for tool in counted {
        if tool.entries > 1 {
            found.push(format!(
                "{LOCK} records {} entries for {}, so which one an install takes is undecided",
                tool.entries, tool.tool
            ));
        }
        if tool.platformless > 0 {
            found.push(format!(
                "{LOCK} records an entry for {} with no platform block, which no rule here reads",
                tool.tool
            ));
        }
    }
    found
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
            "{PINS} does not set locked = true under [settings], so an install takes whatever a registry serves"
        ));
    }
    if !tool_config_locked(pins) {
        found.push(format!(
            "{PINS} does not set locked = true under [tool_config], so MISE_LOCKED_SCOPES turns locked mode off"
        ));
    }
    if !verify_provenance(pins) {
        found.push(format!(
            "{PINS} does not set locked_verify_provenance = true, so an install trusts the attestation {LOCK} records rather than checking it"
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
    let pinned: BTreeSet<&str> = tools.iter().map(|(name, _)| name.as_str()).collect();

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

    found.extend(array_problems(lock));

    // Every platform block is read, not only the ones the matrix names. A block
    // nothing reads is a url and a checksum nobody checked, and `RELOCK` writes
    // exactly the matrix platforms, so any other is an anomaly.
    for entry in &locked_entries {
        if pinned.contains(entry.tool.as_str()) && !platforms.contains(&entry.platform) {
            found.push(format!(
                "{LOCK} records a {} entry for {}, which no matrix runner installs",
                entry.platform, entry.tool
            ));
        }
    }

    for (name, version) in &tools {
        let Some(tool) = tool_for(name) else {
            continue;
        };
        for platform in &platforms {
            if !locked_entries
                .iter()
                .any(|entry| &entry.tool == name && &entry.platform == platform)
            {
                found.push(format!("{name} records no {platform} entry in {LOCK}"));
            }
        }
        for entry in locked_entries.iter().filter(|entry| &entry.tool == name) {
            found.extend(entry_problems(tool, version, entry));
        }
    }

    for name in locked_entries
        .iter()
        .map(|entry| &entry.tool)
        .collect::<BTreeSet<&String>>()
    {
        if !pinned.contains(name.as_str()) {
            found.push(format!("{LOCK} locks {name}, which {PINS} no longer pins"));
        }
    }

    found.sort_unstable();
    found.dedup();
    found
}

#[cfg(test)]
mod tests {
    use super::{
        Backend, TOOLS, absolute, environment_problems, locked, problems, tool_config_locked,
    };
    use std::collections::BTreeSet;

    /// A pin file every rule accepts, for the cases below to change one thing
    /// in.
    const SOUND_PINS: &str = concat!(
        "[tools]\n",
        "taplo = \"0.10.0\"\n",
        "\"github:nextest-rs/nextest\" = { version = \"0.9.145\" }\n",
        "\n[tool_config]\nlocked = true\n",
        "\n[settings]\nlocked = true\nlocked_verify_provenance = true\n",
    );

    /// Two digests of the right shape, for the cases that name one.
    const DIGEST_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const DIGEST_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    /// A lockfile every rule accepts, shaped as mise writes one.
    ///
    /// taplo carries no provenance and nextest carries one, so the sound
    /// document holds both halves of that rule.
    const SOUND_LOCK: &str = concat!(
        "[[tools.taplo]]\nversion = \"0.10.0\"\nbackend = \"aqua:tamasfe/taplo\"\n",
        "[tools.taplo.\"platforms.linux-x64\"]\nchecksum = \"sha256:",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"\n",
        "url = \"https://github.com/tamasfe/taplo/releases/download/0.10.0/taplo-linux-x86_64.gz\"\n",
        "url_api = \"https://api.github.com/repos/tamasfe/taplo/releases/assets/257322600\"\n",
        "[tools.taplo.\"platforms.windows-x64\"]\nchecksum = \"sha256:",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\"\n",
        "url = \"https://github.com/tamasfe/taplo/releases/download/0.10.0/taplo-windows-x86_64.zip\"\n",
        "url_api = \"https://api.github.com/repos/tamasfe/taplo/releases/assets/257323062\"\n",
        "[[tools.\"github:nextest-rs/nextest\"]]\nversion = \"0.9.145\"\n",
        "backend = \"github:nextest-rs/nextest\"\n",
        "[tools.\"github:nextest-rs/nextest\".\"platforms.linux-x64\"]\nchecksum = \"sha256:",
        "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc\"\n",
        "url = \"https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-0.9.145/cargo-nextest-0.9.145-x86_64-unknown-linux-gnu.tar.gz\"\n",
        "url_api = \"https://api.github.com/repos/nextest-rs/nextest/releases/assets/568838794\"\n",
        "provenance = \"github-attestations\"\n",
        "[tools.\"github:nextest-rs/nextest\".\"platforms.windows-x64\"]\nchecksum = \"sha256:",
        "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd\"\n",
        "url = \"https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-0.9.145/cargo-nextest-0.9.145-x86_64-pc-windows-msvc.zip\"\n",
        "url_api = \"https://api.github.com/repos/nextest-rs/nextest/releases/assets/568848390\"\n",
        "provenance = \"github-attestations\"\n",
    );

    /// A matrix naming both runners the gate installs on.
    const SOUND_WORKFLOW: &str = "        os: [windows-latest, ubuntu-latest]\n";

    /// The sound documents pass, so every refusal below changes one thing from
    /// something that worked.
    #[test]
    fn the_sound_documents_meet_every_rule() {
        assert_eq!(
            problems(SOUND_PINS, SOUND_LOCK, SOUND_WORKFLOW),
            Vec::<String>::new()
        );
    }

    /// Run each case and assert the rules said the thing it names.
    ///
    /// A case is the sound trio with one thing changed, so a rule that goes
    /// quiet shows up as the sentence nobody produced.
    fn refuses(cases: &[(&str, String, String, String, &str)]) {
        for (what, pins, lock, workflow, wanted) in cases {
            let found = problems(pins, lock, workflow);
            assert!(
                found.iter().any(|problem| problem.contains(wanted)),
                "{what}: nothing said {wanted:?}, got {found:?}"
            );
        }
    }

    /// The rules refuse each shape they name, against documents written here.
    ///
    /// A rule nothing can break is a rule that proves nothing, so every case is
    /// the sound trio with one thing changed. The url and backend cases are the
    /// ones a checksum rule alone accepts: each leaves the lockfile agreeing
    /// with itself and disagreeing with `TOOLS`.
    #[test]
    fn the_pin_rules_refuse_the_shapes_they_name() {
        let cases: &[(&str, String, String, String, &str)] = &[
            (
                "locked mode off",
                SOUND_PINS.replace(
                    "\n[settings]\nlocked = true\n",
                    "\n[settings]\nlocked = false\n",
                ),
                SOUND_LOCK.to_string(),
                SOUND_WORKFLOW.to_string(),
                "does not set locked = true under [settings]",
            ),
            (
                "the tool_config locked line deleted",
                SOUND_PINS.replace("\n[tool_config]\nlocked = true\n", ""),
                SOUND_LOCK.to_string(),
                SOUND_WORKFLOW.to_string(),
                "does not set locked = true under [tool_config]",
            ),
        ];
        refuses(cases);
    }

    /// The rules refuse a lockfile that drops or invents an attestation, holds
    /// two answers, or records a platform nothing installs.
    ///
    /// Each of these is a line added to or removed from the generated file,
    /// which is the diff a dependency bot produces and a reviewer skims.
    #[test]
    fn the_lockfile_rules_refuse_an_entry_nothing_else_reads() {
        let cases: &[(&str, String, String, String, &str)] = &[
            (
                "the provenance verification line deleted",
                SOUND_PINS.replace("\nlocked_verify_provenance = true\n", "\n"),
                SOUND_LOCK.to_string(),
                SOUND_WORKFLOW.to_string(),
                "does not set locked_verify_provenance = true",
            ),
            (
                "an attested tool's provenance line deleted",
                SOUND_PINS.to_string(),
                SOUND_LOCK.replace("provenance = \"github-attestations\"\n", ""),
                SOUND_WORKFLOW.to_string(),
                "records no provenance, and github-attestations is required for it",
            ),
            (
                "a provenance line added to a tool with no attestation",
                SOUND_PINS.to_string(),
                SOUND_LOCK.replace(
                    "url_api = \"https://api.github.com/repos/tamasfe/taplo/releases/assets/257322600\"\n",
                    "url_api = \"https://api.github.com/repos/tamasfe/taplo/releases/assets/257322600\"\nprovenance = \"github-attestations\"\n",
                ),
                SOUND_WORKFLOW.to_string(),
                "records provenance github-attestations, and no release of it is attested",
            ),
        ];
        refuses(cases);
    }

    /// The rules refuse a lockfile holding more than one answer for a tool.
    ///
    /// A second array entry is legal TOML, and one carrying no platform block
    /// produces no row for the per-platform rules to read, so the count is
    /// taken from the array itself.
    #[test]
    fn the_lockfile_rules_refuse_a_file_with_two_answers() {
        let cases: &[(&str, String, String, String, &str)] = &[
            (
                "a second entry for a tool, beside the sound one",
                SOUND_PINS.to_string(),
                format!(
                    "{SOUND_LOCK}[[tools.taplo]]\nversion = \"0.10.0\"\nbackend = \"aqua:attacker/taplo\"\n[tools.taplo.\"platforms.linux-x64\"]\nchecksum = \"sha256:{DIGEST_A}\"\nurl = \"https://cdn.attacker.example/taplo/payload.gz\"\n"
                ),
                SOUND_WORKFLOW.to_string(),
                "records 2 entries for taplo",
            ),
            (
                "a second entry carrying no platform block",
                SOUND_PINS.to_string(),
                format!(
                    "{SOUND_LOCK}[[tools.taplo]]\nversion = \"0.10.0\"\nbackend = \"aqua:attacker/taplo\"\n"
                ),
                SOUND_WORKFLOW.to_string(),
                "records an entry for taplo with no platform block",
            ),
            (
                "a second entry carrying only an options table",
                SOUND_PINS.to_string(),
                format!(
                    "{SOUND_LOCK}[[tools.taplo]]\nversion = \"9.9.9\"\nbackend = \"aqua:attacker/taplo\"\n[tools.taplo.options]\nversion_prefix = \"attacker-\"\n"
                ),
                SOUND_WORKFLOW.to_string(),
                "records an entry for taplo with no platform block",
            ),
            (
                "a platform block no matrix runner installs",
                SOUND_PINS.to_string(),
                format!(
                    "{SOUND_LOCK}[tools.taplo.\"platforms.macos-arm64\"]\nchecksum = \"md5:00\"\nurl = \"https://cdn.attacker.example/taplo/payload.gz\"\n"
                ),
                SOUND_WORKFLOW.to_string(),
                "records a macos-arm64 entry for taplo, which no matrix runner installs",
            ),
            (
                "a truncated digest",
                SOUND_PINS.to_string(),
                SOUND_LOCK.replace(&format!("sha256:{DIGEST_A}"), "sha256:0"),
                SOUND_WORKFLOW.to_string(),
                "and a sha256 digest is 64 hex digits",
            ),
            (
                "a checksum dropped, the platform block kept",
                SOUND_PINS.to_string(),
                SOUND_LOCK.replace(&format!("checksum = \"sha256:{DIGEST_B}\"\n"), ""),
                SOUND_WORKFLOW.to_string(),
                "taplo on windows-x64 carries no checksum",
            ),
            (
                "a platform block dropped",
                SOUND_PINS.to_string(),
                SOUND_LOCK.replace(
                    &format!(
                        "[tools.taplo.\"platforms.linux-x64\"]\nchecksum = \"sha256:{DIGEST_A}\"\n"
                    ),
                    "",
                ),
                SOUND_WORKFLOW.to_string(),
                "taplo records no linux-x64 entry",
            ),
            (
                "the two files disagreeing on a version",
                SOUND_PINS.replace("taplo = \"0.10.0\"", "taplo = \"0.10.1\""),
                SOUND_LOCK.to_string(),
                SOUND_WORKFLOW.to_string(),
                "pins taplo 0.10.1 and mise.lock records 0.10.0",
            ),
            (
                "a range in place of an exact release",
                SOUND_PINS.replace("taplo = \"0.10.0\"", "taplo = \"0.10\""),
                SOUND_LOCK.to_string(),
                SOUND_WORKFLOW.to_string(),
                "which is not one exact release",
            ),
            (
                "a lockfile entry the pin file no longer names",
                SOUND_PINS.replace("taplo = \"0.10.0\"\n", ""),
                SOUND_LOCK.to_string(),
                SOUND_WORKFLOW.to_string(),
                "locks taplo, which mise.toml no longer pins",
            ),
            (
                "a runner leg with no platform",
                SOUND_PINS.to_string(),
                SOUND_LOCK.to_string(),
                SOUND_WORKFLOW.replace("ubuntu-latest", "macos-latest"),
                "macos-latest has no mise platform",
            ),
            (
                "a key naming no binary",
                SOUND_PINS.replace("github:nextest-rs/nextest", "github:nextest-rs/elsewhere"),
                SOUND_LOCK.replace("github:nextest-rs/nextest", "github:nextest-rs/elsewhere"),
                SOUND_WORKFLOW.to_string(),
                "is a key no entry names a binary and a release for",
            ),
        ];
        refuses(cases);
    }

    /// The rules refuse an entry whose artifact moved, which a checksum rule
    /// alone accepts.
    ///
    /// Each case leaves the lockfile agreeing with itself and disagreeing with
    /// `TOOLS`, which is the whole point of holding the generated file to a
    /// table in source.
    #[test]
    fn the_lockfile_rules_refuse_a_moved_artifact() {
        let cases: &[(&str, String, String, String, &str)] = &[
            (
                "the backend moved to another account",
                SOUND_PINS.to_string(),
                SOUND_LOCK.replace("aqua:tamasfe/taplo", "aqua:attacker/taplo"),
                SOUND_WORKFLOW.to_string(),
                "installs through aqua:attacker/taplo, and mise.toml pins aqua:tamasfe/taplo",
            ),
            (
                "the backend switched to the other kind",
                SOUND_PINS.to_string(),
                SOUND_LOCK.replace("aqua:tamasfe/taplo", "github:tamasfe/taplo"),
                SOUND_WORKFLOW.to_string(),
                "installs through github:tamasfe/taplo",
            ),
            (
                "the backend line dropped",
                SOUND_PINS.to_string(),
                SOUND_LOCK.replace("backend = \"aqua:tamasfe/taplo\"\n", ""),
                SOUND_WORKFLOW.to_string(),
                "installs through no backend",
            ),
        ];
        refuses(cases);
    }

    /// The rules refuse a url that moved, which a checksum rule alone accepts.
    ///
    /// The host, the owner and the release each come from `TOOLS` rather than
    /// from the generated file, so no rewrite confined to that file satisfies
    /// them.
    #[test]
    fn the_lockfile_rules_refuse_a_moved_url() {
        let cases: &[(&str, String, String, String, &str)] = &[
            (
                "the url pointed at another host",
                SOUND_PINS.to_string(),
                SOUND_LOCK.replace(
                    "https://github.com/tamasfe/taplo/releases/download/0.10.0/taplo-linux-x86_64.gz",
                    "https://cdn.attacker.example/taplo/taplo-linux-x86_64.gz",
                ),
                SOUND_WORKFLOW.to_string(),
                "records a url served by cdn.attacker.example, and github.com serves it",
            ),
            (
                "the url on a host whose text holds the real one",
                SOUND_PINS.to_string(),
                SOUND_LOCK.replace(
                    "https://github.com/tamasfe",
                    "https://github.com.attacker.example/tamasfe",
                ),
                SOUND_WORKFLOW.to_string(),
                "records a url served by github.com.attacker.example",
            ),
            (
                "the url under another owner",
                SOUND_PINS.to_string(),
                SOUND_LOCK.replace(
                    "github.com/tamasfe/taplo/releases",
                    "github.com/attacker/taplo/releases",
                ),
                SOUND_WORKFLOW.to_string(),
                "and /tamasfe/taplo/releases/download/ holds it",
            ),
            (
                "the url naming an older release of the genuine repository",
                SOUND_PINS.to_string(),
                SOUND_LOCK.replace("download/0.10.0/taplo-linux", "download/0.9.0/taplo-linux"),
                SOUND_WORKFLOW.to_string(),
                "records a url tagged 0.9.0, and mise.lock records the release 0.10.0",
            ),
            (
                "an older tag with the pinned release moved into the filename",
                SOUND_PINS.to_string(),
                SOUND_LOCK.replace(
                    "download/0.10.0/taplo-linux-x86_64.gz",
                    "download/0.9.0/taplo-0.10.0-linux-x86_64.gz",
                ),
                SOUND_WORKFLOW.to_string(),
                "records a url tagged 0.9.0, and mise.lock records the release 0.10.0",
            ),
            (
                "a prerelease tag that starts with the pinned release",
                SOUND_PINS.to_string(),
                SOUND_LOCK.replace("download/0.10.0/taplo-linux", "download/0.10.0-rc1/taplo-linux"),
                SOUND_WORKFLOW.to_string(),
                "records a url tagged 0.10.0-rc1, and mise.lock records the release 0.10.0",
            ),
            (
                "the url walked back out of the release path",
                SOUND_PINS.to_string(),
                SOUND_LOCK.replace(
                    "download/0.10.0/taplo-linux-x86_64.gz",
                    "download/0.10.0/../../../../attacker/taplo/0.10.0.gz",
                ),
                SOUND_WORKFLOW.to_string(),
                "which is not an absolute https url",
            ),
            (
                "the url served over plain http",
                SOUND_PINS.to_string(),
                SOUND_LOCK.replace(
                    "url = \"https://github.com/tamasfe",
                    "url = \"http://github.com/tamasfe",
                ),
                SOUND_WORKFLOW.to_string(),
                "which is not an absolute https url",
            ),
            (
                "the url carrying userinfo ahead of the real host",
                SOUND_PINS.to_string(),
                SOUND_LOCK.replace(
                    "https://github.com/tamasfe",
                    "https://github.com@attacker.example/tamasfe",
                ),
                SOUND_WORKFLOW.to_string(),
                "which is not an absolute https url",
            ),
            (
                "the url dropped",
                SOUND_PINS.to_string(),
                SOUND_LOCK.replace(
                    "url = \"https://github.com/tamasfe/taplo/releases/download/0.10.0/taplo-linux-x86_64.gz\"\n",
                    "",
                ),
                SOUND_WORKFLOW.to_string(),
                "records no url",
            ),
        ];
        refuses(cases);
    }

    /// The rules refuse an api reference that moved, which nothing downstream
    /// would notice.
    ///
    /// The api addresses a release by asset number rather than by tag, so the
    /// version rule does not apply and the host and the owner carry it alone.
    #[test]
    fn the_lockfile_rules_refuse_a_moved_api_reference() {
        let cases: &[(&str, String, String, String, &str)] = &[
            (
                "the api reference moved to another host",
                SOUND_PINS.to_string(),
                SOUND_LOCK.replace(
                    "https://api.github.com/repos/tamasfe",
                    "https://api.github.com.attacker.example/repos/tamasfe",
                ),
                SOUND_WORKFLOW.to_string(),
                "api records a url served by api.github.com.attacker.example",
            ),
            (
                "the api reference under another owner",
                SOUND_PINS.to_string(),
                SOUND_LOCK.replace(
                    "api.github.com/repos/tamasfe/taplo",
                    "api.github.com/repos/attacker/taplo",
                ),
                SOUND_WORKFLOW.to_string(),
                "and /repos/tamasfe/taplo/releases/ holds it",
            ),
            (
                "the api reference dropped",
                SOUND_PINS.to_string(),
                SOUND_LOCK.replace(
                    "url_api = \"https://api.github.com/repos/tamasfe/taplo/releases/assets/257322600\"\n",
                    "",
                ),
                SOUND_WORKFLOW.to_string(),
                "records no url_api",
            ),
        ];

        refuses(cases);
    }

    /// The url reader refuses every url that is not a plain absolute https one.
    ///
    /// Each of these holds the text a search for the host would find, and each
    /// lands somewhere else.
    #[test]
    fn the_url_reader_refuses_what_a_text_match_accepts() {
        for text in [
            "http://github.com/a/b",
            "https://github.com@attacker.example/a/b",
            "https://github.com:8443/a/b",
            "https://github.com/a/b/../../c",
            "https://github.com/a/b?github.com/x",
            "https://github.com/a/b#github.com/x",
            "https://github.com\\attacker.example/a/b",
            "https://%67ithub.com/a/b",
            "https://github.com",
            "github.com/a/b",
        ] {
            assert!(
                absolute(text).is_none(),
                "{text} was read as an absolute url"
            );
        }
        let parsed = absolute("https://github.com/a/b").expect("a plain release url");
        assert_eq!(parsed.host, "github.com");
        assert_eq!(parsed.path, "/a/b");
    }

    /// A host differing only in case is the same host, and one differing by a
    /// label is not.
    #[test]
    fn the_host_comparison_ignores_case_and_nothing_else() {
        let same = absolute("https://GitHub.COM/a/b").expect("a url");
        assert!(same.host.eq_ignore_ascii_case("github.com"));
        let other = absolute("https://github.com.attacker.example/a/b").expect("a url");
        assert!(!other.host.eq_ignore_ascii_case("github.com"));
    }

    /// Each backend renders the coordinate mise records for it.
    ///
    /// The two kinds differ only in the word ahead of the colon, and both carry
    /// the owner and the repository. An assertion written for one shape alone
    /// would pass the other by.
    #[test]
    fn each_backend_renders_the_coordinate_mise_records() {
        let aqua = TOOLS
            .iter()
            .find(|tool| tool.key == "taplo")
            .expect("taplo is pinned");
        assert_eq!(aqua.coordinate(), "aqua:tamasfe/taplo");
        assert_eq!(aqua.release_prefix(), "/tamasfe/taplo/releases/download/");
        assert_eq!(aqua.api_prefix(), "/repos/tamasfe/taplo/releases/");

        let github = TOOLS
            .iter()
            .find(|tool| tool.key == "github:rustsec/rustsec")
            .expect("rustsec is pinned");
        assert_eq!(github.coordinate(), "github:rustsec/rustsec");
        assert_eq!(github.backend, Backend::Github);
    }

    /// No two entries share a key or a binary, and every one names a release.
    #[test]
    fn the_tool_table_repeats_no_key_and_no_binary() {
        let keys: BTreeSet<&str> = TOOLS.iter().map(|tool| tool.key).collect();
        assert_eq!(keys.len(), TOOLS.len(), "two entries share a key");
        let binaries: BTreeSet<&str> = TOOLS.iter().map(|tool| tool.binary).collect();
        assert_eq!(binaries.len(), TOOLS.len(), "two entries share a binary");
        for tool in TOOLS {
            assert!(!tool.owner.is_empty(), "{} names no owner", tool.key);
            assert!(
                !tool.repository.is_empty(),
                "{} names no repository",
                tool.key
            );
        }
    }

    /// The two locked spellings are read from their own tables.
    ///
    /// `settings.locked` answers to the environment and `tool_config.locked`
    /// does not, so a file setting one is not a file setting the other.
    #[test]
    fn each_locked_spelling_is_read_from_its_own_table() {
        let settings_only = "[tools]\na = \"1.0.0\"\n[settings]\nlocked = true\n";
        assert!(locked(settings_only));
        assert!(!tool_config_locked(settings_only));

        let tool_config_only = "[tools]\na = \"1.0.0\"\n[tool_config]\nlocked = true\n";
        assert!(!locked(tool_config_only));
        assert!(tool_config_locked(tool_config_only));
    }

    /// The environment rule refuses every value that leaves mise reading a
    /// second document.
    ///
    /// Membership is the wrong question for a filename list. mise takes the
    /// last name in the list, so a value naming the pin file first and another
    /// file second reads the other file, while a value naming them the other
    /// way round is harmless. A rule asking whether the pin file appears
    /// accepts both, which is what makes it look right when only the harmless
    /// order is tried.
    #[test]
    fn the_environment_rule_refuses_a_value_naming_a_second_file() {
        let nothing_set = environment_problems(|_| None);
        assert_eq!(nothing_set, Vec::<String>::new());

        let only_ours = environment_problems(|name| {
            (name == "MISE_OVERRIDE_CONFIG_FILENAMES").then(|| "mise.toml".to_string())
        });
        assert_eq!(only_ours, Vec::<String>::new());

        for value in [
            "mise.toml:evil.toml",
            "evil.toml:mise.toml",
            "evil.toml",
            "mise.toml: evil.toml",
            "",
        ] {
            let found = environment_problems(|name| {
                (name == "MISE_OVERRIDE_CONFIG_FILENAMES").then(|| value.to_string())
            });
            assert!(
                !found.is_empty(),
                "a filename list of {value:?} was accepted"
            );
        }

        // Any value adds a file, so the rule reads the variable rather than
        // what it is set to.
        for value in ["evil", "production", "mise"] {
            let found =
                environment_problems(|name| (name == "MISE_ENV").then(|| value.to_string()));
            assert!(
                found.iter().any(|problem| problem.contains("MISE_ENV")),
                "MISE_ENV set to {value:?} was accepted"
            );
        }
    }
}
