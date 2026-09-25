//! The files that pin every tool mise installs here, and the rules they meet.
//!
//! `mise.toml` holds one version per tool and `mise.lock` holds a checksum, a
//! url and a backend per platform for each of them. `mise.semver.toml` and
//! `mise.semver.lock` are the same pair for cargo-semver-checks alone, which
//! mise loads only where `MISE_ENV=semver`. These rules run before the gate
//! runs any tool, because a lockfile entry decides what an install downloads:
//! mise's locked mode fetches the url the entry records rather than asking the
//! backend, and it compares the checksum the entry records. A rule that runs
//! after the install reports a finding about a binary that already executed.
//!
//! A checksum rule alone holds the lockfile to itself. The url, the backend and
//! the checksum all sit in the generated file, so an edit that moves all three
//! together leaves every one of them agreeing. What the url and backend rules
//! add is a second document to disagree with: the owner and the repository each
//! artifact belongs to live in [`TOOLS`], in source, so an artifact moving to
//! another host or another account takes an edit a reviewer reads.
//!
//! The url is compared whole against one built from that table, with no url
//! parser in between, because every parser reads some url differently from the
//! one mise fetches. The api reference beside it names an asset by number
//! alone, so nothing offline binds it to a release, and mise fetches it
//! whenever a HEAD on the url fails. [`PINS`] therefore carries a
//! `url_replacements` rule sending every such request to a host that resolves
//! nowhere, and every workflow install carries the same map in
//! `MISE_URL_REPLACEMENTS`, which a committed config file cannot lift.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use crate::tree::printable;

/// The file pinning a version for every tool mise installs.
pub const PINS: &str = "mise.toml";

/// The file holding a checksum, a url and a backend per platform for every tool
/// [`PINS`] pins.
pub const LOCK: &str = "mise.lock";

/// The command that rewrites [`LOCK`] after an edit to [`PINS`], for the
/// platforms `lockfile_platforms` names there.
pub const RELOCK: &str = "mise lock";

/// A pin file, the lockfile mise writes for it, and the command that rewrites
/// that lockfile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pair {
    /// The file pinning one version per tool.
    pub pins: &'static str,
    /// The file holding a checksum, a url and a backend per platform for each.
    pub lock: &'static str,
    /// The command that rewrites `lock` after an edit to `pins`.
    pub relock: &'static str,
}

/// [`PINS`] and [`LOCK`], which every job loads.
pub const MAIN: Pair = Pair {
    pins: PINS,
    lock: LOCK,
    relock: RELOCK,
};

/// The pair holding cargo-semver-checks, which mise loads beside [`MAIN`] only
/// where `MISE_ENV=semver`.
///
/// A mise shim installs any configured tool the first time another shim's
/// process looks for it, so a job that must never run the semver check keeps
/// it out of every configuration it loads. Its platforms and its locked mode
/// come from [`PINS`].
pub const SEMVER: Pair = Pair {
    pins: "mise.semver.toml",
    lock: "mise.semver.lock",
    relock: "MISE_ENV=semver mise lock",
};

/// Every pair, in the order the rules read them.
pub const PAIRS: &[Pair] = &[MAIN, SEMVER];

/// The host every release artifact [`LOCK`] records is served from.
pub const RELEASE_HOST: &str = "github.com";

/// The host every release api reference [`LOCK`] records is served from.
pub const API_HOST: &str = "api.github.com";

/// The provenance [`LOCK`] records for a release carrying an attestation.
pub const ATTESTED: &str = "github-attestations";

/// The one `url_replacements` key [`PINS`] carries under `[settings]`, which
/// matches every release asset api reference.
pub const URL_API_PATTERN: &str =
    r"regex:^https://api\.github\.com/repos/[^/]+/[^/]+/releases/assets/.*$";

/// Where [`URL_API_PATTERN`] sends a request: a reserved name that resolves
/// nowhere, so an install that falls back to the api fails.
pub const URL_API_REFUSED: &str = "https://url-api-refused.invalid/";

/// The digits a sha256 digest is written with, after its `sha256:` prefix.
const SHA256_DIGITS: usize = 64;

/// The tables [`PINS`] holds. mise also runs hooks and tasks and exports an
/// environment from a pin file, and no rule here reads any of those.
const PIN_TABLES: &[&str] = &["tools", "tool_config", "settings"];

/// The settings [`PINS`] sets true, each a refusal mise makes: the locked mode,
/// the lockfile itself, the attestation checks and their failure mode.
const SETTINGS_ON: &[&str] = &[
    "locked",
    "lockfile",
    "locked_verify_provenance",
    "provenance_api_failures_fatal",
    "github_attestations",
];

/// The settings [`PINS`] holds beside [`SETTINGS_ON`], each read by a rule here.
const SETTINGS_READ: &[&str] = &["lockfile_platforms", "url_replacements", "aqua"];

/// The settings [`PINS`] sets true under `[settings.aqua]`: the attestation
/// check for a tool the aqua backend installs. It defaults on, and the file
/// names it so a changed default cannot turn it off under the repository.
const AQUA_SETTINGS_ON: &[&str] = &["github_attestations"];

/// The keys mise writes at a lockfile's top level.
const LOCK_KEYS: &[&str] = &["lockfile_version", "tools"];

/// The lockfile format every rule here reads. mise writes the number at the
/// top of the file, and another format could carry what no rule knows about.
const LOCKFILE_VERSION: i64 = 1;

/// The keys mise writes on a locked entry beside its platform blocks.
const ENTRY_KEYS: &[&str] = &["version", "backend", "specifiers", "options"];

/// The keys mise writes in a platform block.
const PLATFORM_KEYS: &[&str] = &["checksum", "url", "url_api", "provenance"];

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

/// The file one platform installs from a tool's release.
pub struct Asset {
    /// The mise platform the file installs on.
    pub platform: &'static str,
    /// The file name, with `{version}` standing for the pinned release.
    pub name: &'static str,
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
    /// The pair whose pin file holds this tool, and no other.
    pub pair: Pair,
    /// The file each platform installs, one per platform `lockfile_platforms`
    /// names.
    ///
    /// Held here so the url [`LOCK`] records is compared whole against one
    /// built from source. An upstream that renames an asset takes an edit here
    /// in the same diff as the relock.
    pub assets: &'static [Asset],
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

    /// The url [`LOCK`] has to record for `version` on `platform`, or `None`
    /// where [`Tool::assets`] names no file for that platform.
    #[must_use]
    pub fn url(&self, platform: &str, version: &str) -> Option<String> {
        let asset = self
            .assets
            .iter()
            .find(|asset| asset.platform == platform)?;
        Some(format!(
            "https://{RELEASE_HOST}/{}/{}/releases/download/{}/{}",
            self.owner,
            self.repository,
            self.tag(version),
            asset.name.replace("{version}", version)
        ))
    }

    /// The text every release api reference for this tool starts with, ahead
    /// of the asset number.
    ///
    /// The api addresses an asset by number rather than by tag, so nothing here
    /// carries the version. GitHub answers a number under another repository's
    /// path with a 404, which is what binds the repository.
    #[must_use]
    pub fn api_prefix(&self) -> String {
        format!(
            "https://{API_HOST}/repos/{}/{}/releases/assets/",
            self.owner, self.repository
        )
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

/// Every tool mise installs here, with the release each one's artifacts come
/// from and the pair that pins it.
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
        pair: MAIN,
        assets: &[
            Asset {
                platform: "linux-x64",
                name: "actionlint_{version}_linux_amd64.tar.gz",
            },
            Asset {
                platform: "macos-arm64",
                name: "actionlint_{version}_darwin_arm64.tar.gz",
            },
            Asset {
                platform: "windows-x64",
                name: "actionlint_{version}_windows_amd64.zip",
            },
        ],
    },
    Tool {
        key: "cargo-deny",
        binary: "cargo-deny",
        backend: Backend::Aqua,
        owner: "EmbarkStudios",
        repository: "cargo-deny",
        tag_prefix: "",
        provenance: None,
        pair: MAIN,
        assets: &[
            Asset {
                platform: "linux-x64",
                name: "cargo-deny-{version}-x86_64-unknown-linux-musl.tar.gz",
            },
            Asset {
                platform: "macos-arm64",
                name: "cargo-deny-{version}-aarch64-apple-darwin.tar.gz",
            },
            Asset {
                platform: "windows-x64",
                name: "cargo-deny-{version}-x86_64-pc-windows-msvc.tar.gz",
            },
        ],
    },
    Tool {
        key: "github:bnjbvr/cargo-machete",
        binary: "cargo-machete",
        backend: Backend::Github,
        owner: "bnjbvr",
        repository: "cargo-machete",
        tag_prefix: "v",
        provenance: None,
        pair: MAIN,
        assets: &[
            Asset {
                platform: "linux-x64",
                name: "cargo-machete-v{version}-x86_64-unknown-linux-musl.tar.gz",
            },
            Asset {
                platform: "macos-arm64",
                name: "cargo-machete-v{version}-aarch64-apple-darwin.tar.gz",
            },
            Asset {
                platform: "windows-x64",
                name: "cargo-machete-v{version}-x86_64-pc-windows-msvc.tar.gz",
            },
        ],
    },
    Tool {
        key: "github:nextest-rs/nextest",
        binary: "cargo-nextest",
        backend: Backend::Github,
        owner: "nextest-rs",
        repository: "nextest",
        tag_prefix: "cargo-nextest-",
        provenance: Some(ATTESTED),
        pair: MAIN,
        assets: &[
            Asset {
                platform: "linux-x64",
                name: "cargo-nextest-{version}-x86_64-unknown-linux-gnu.tar.gz",
            },
            Asset {
                platform: "macos-arm64",
                name: "cargo-nextest-{version}-universal-apple-darwin.tar.gz",
            },
            Asset {
                platform: "windows-x64",
                name: "cargo-nextest-{version}-x86_64-pc-windows-msvc.zip",
            },
        ],
    },
    Tool {
        key: "github:obi1kenobi/cargo-semver-checks",
        binary: "cargo-semver-checks",
        backend: Backend::Github,
        owner: "obi1kenobi",
        repository: "cargo-semver-checks",
        tag_prefix: "v",
        provenance: None,
        pair: SEMVER,
        assets: &[
            Asset {
                platform: "linux-x64",
                name: "cargo-semver-checks-x86_64-unknown-linux-gnu.tar.gz",
            },
            Asset {
                platform: "macos-arm64",
                name: "cargo-semver-checks-aarch64-apple-darwin.tar.gz",
            },
            Asset {
                platform: "windows-x64",
                name: "cargo-semver-checks-x86_64-pc-windows-msvc.zip",
            },
        ],
    },
    Tool {
        key: "release-plz",
        binary: "release-plz",
        backend: Backend::Aqua,
        owner: "release-plz",
        repository: "release-plz",
        tag_prefix: "release-plz-v",
        provenance: None,
        pair: MAIN,
        assets: &[
            Asset {
                platform: "linux-x64",
                name: "release-plz-x86_64-unknown-linux-gnu.tar.gz",
            },
            Asset {
                platform: "macos-arm64",
                name: "release-plz-aarch64-apple-darwin.tar.gz",
            },
            Asset {
                platform: "windows-x64",
                name: "release-plz-x86_64-pc-windows-msvc.tar.gz",
            },
        ],
    },
    Tool {
        key: "shellcheck",
        binary: "shellcheck",
        backend: Backend::Aqua,
        owner: "koalaman",
        repository: "shellcheck",
        tag_prefix: "v",
        provenance: None,
        pair: MAIN,
        assets: &[
            Asset {
                platform: "linux-x64",
                name: "shellcheck-v{version}.linux.x86_64.tar.xz",
            },
            Asset {
                platform: "macos-arm64",
                name: "shellcheck-v{version}.darwin.aarch64.tar.xz",
            },
            Asset {
                platform: "windows-x64",
                name: "shellcheck-v{version}.zip",
            },
        ],
    },
    Tool {
        key: "taplo",
        binary: "taplo",
        backend: Backend::Aqua,
        owner: "tamasfe",
        repository: "taplo",
        tag_prefix: "",
        provenance: None,
        pair: MAIN,
        assets: &[
            Asset {
                platform: "linux-x64",
                name: "taplo-linux-x86_64.gz",
            },
            Asset {
                platform: "macos-arm64",
                name: "taplo-darwin-aarch64.gz",
            },
            Asset {
                platform: "windows-x64",
                name: "taplo-windows-x86_64.zip",
            },
        ],
    },
    Tool {
        key: "zizmor",
        binary: "zizmor",
        backend: Backend::Aqua,
        owner: "zizmorcore",
        repository: "zizmor",
        tag_prefix: "v",
        provenance: Some(ATTESTED),
        pair: MAIN,
        assets: &[
            Asset {
                platform: "linux-x64",
                name: "zizmor-x86_64-unknown-linux-gnu.tar.gz",
            },
            Asset {
                platform: "macos-arm64",
                name: "zizmor-aarch64-apple-darwin.tar.gz",
            },
            Asset {
                platform: "windows-x64",
                name: "zizmor-x86_64-pc-windows-msvc.zip",
            },
        ],
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
    /// How many of those carry a nested `platforms` table.
    ///
    /// mise writes each platform as a quoted `platforms.<name>` key, and it
    /// also installs from the nested form, for any platform, while the rules
    /// here read the quoted keys alone.
    pub nested: usize,
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
pub fn pinned_tools(pair: Pair, text: &str) -> Result<Vec<(String, String)>, String> {
    let pins = pair.pins;
    let document: toml::Value =
        toml::from_str(text).map_err(|err| format!("{pins} is not TOML: {err}"))?;
    let tools = document
        .get("tools")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| format!("{pins} holds no tools table"))?;
    tools
        .keys()
        .map(|name| {
            let version = pinned_version(text, name)
                .ok_or_else(|| format!("{pins} pins no version for {name}"))?;
            Ok((name.clone(), version))
        })
        .collect()
}

/// The platforms [`PINS`] names under `settings.lockfile_platforms`, which are
/// the platforms [`LOCK`] has to carry a block for, per tool, and no other.
///
/// mise writes exactly these when it relocks, so a block for any other platform
/// is a url and a checksum nothing installs.
///
/// # Errors
///
/// Returns the sentence a result row carries when the text is not TOML or
/// names no platform list.
pub fn lockfile_platforms(text: &str) -> Result<Vec<String>, String> {
    let document: toml::Value =
        toml::from_str(text).map_err(|err| format!("{PINS} is not TOML: {err}"))?;
    let entries = document
        .get("settings")
        .and_then(|settings| settings.get("lockfile_platforms"))
        .and_then(toml::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut platforms = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let Some(platform) = entry.as_str() else {
            return Err(format!(
                "{PINS} names lockfile_platforms entry {index} as something other than a platform name"
            ));
        };
        platforms.push(platform.to_string());
    }
    if platforms.is_empty() {
        return Err(format!(
            "{PINS} names no lockfile_platforms under [settings], so nothing says which blocks {LOCK} has to carry"
        ));
    }
    Ok(platforms)
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
        .ok_or_else(|| format!("{name:?} is a key no entry names a binary and a release for"))
}

/// Every binary the gate runs, derived from the keys [`PINS`] holds.
///
/// # Errors
///
/// Returns the first sentence [`binary_for`] or [`pinned_tools`] produces.
pub fn pinned_binaries(pair: Pair, text: &str) -> Result<BTreeSet<String>, String> {
    pinned_tools(pair, text)?
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
pub fn locked_platforms(pair: Pair, text: &str) -> Result<Vec<LockedEntry>, String> {
    let lock = pair.lock;
    let document: toml::Value =
        toml::from_str(text).map_err(|err| format!("{lock} is not TOML: {err}"))?;
    let tools = document
        .get("tools")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| format!("{lock} holds no tools table"))?;
    let mut found = Vec::new();
    for (name, entries) in tools {
        let entries = entries
            .as_array()
            .ok_or_else(|| format!("{lock} holds {name} as something other than entries"))?;
        for entry in entries {
            let table = entry.as_table().ok_or_else(|| {
                format!("{lock} holds a {name} entry as something other than a table")
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
pub fn locked_tools(pair: Pair, text: &str) -> Result<Vec<LockedTool>, String> {
    let lock = pair.lock;
    let document: toml::Value =
        toml::from_str(text).map_err(|err| format!("{lock} is not TOML: {err}"))?;
    let tools = document
        .get("tools")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| format!("{lock} holds no tools table"))?;
    let mut found = Vec::new();
    for (name, entries) in tools {
        let entries = entries
            .as_array()
            .ok_or_else(|| format!("{lock} holds {name} as something other than entries"))?;
        let platformless = entries
            .iter()
            .filter(|entry| {
                entry
                    .as_table()
                    .is_none_or(|table| !table.keys().any(|key| key.starts_with("platforms.")))
            })
            .count();
        let nested = entries
            .iter()
            .filter(|entry| {
                entry
                    .as_table()
                    .is_some_and(|table| table.contains_key("platforms"))
            })
            .count();
        found.push(LockedTool {
            tool: name.clone(),
            entries: entries.len(),
            platformless,
            nested,
        });
    }
    Ok(found)
}

/// Whether `version` is one exact release: three dot-separated runs of digits.
///
/// The version is written into the url the lockfile entry is compared against,
/// so anything else could carry a `/` or a `..` that walks the built url out of
/// the release, and the comparison would then hold two equal wrong urls.
fn exact_release(version: &str) -> bool {
    version.split('.').count() == 3
        && version
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

/// Every way one platform entry falls short, as one sentence each.
fn entry_problems(pair: Pair, tool: &Tool, version: &str, entry: &LockedEntry) -> Vec<String> {
    let (pins, lock) = (pair.pins, pair.lock);
    let label = format!("{} on {}", tool.key, entry.platform);
    let mut found = Vec::new();

    match &entry.checksum {
        None => found.push(format!("{label} carries no checksum in {lock}")),
        Some(checksum) => {
            let digits = checksum.strip_prefix("sha256:").unwrap_or_default();
            let sound = digits.len() == SHA256_DIGITS
                && digits.bytes().all(|byte| byte.is_ascii_hexdigit());
            if !sound {
                found.push(format!(
                    "{label} carries {checksum:?}, and a sha256 digest is {SHA256_DIGITS} hex digits"
                ));
            }
        }
    }
    if entry.version != version {
        found.push(format!(
            "{pins} pins {} {version:?} and {lock} records {:?}",
            tool.key, entry.version
        ));
    }

    // `printable` escapes every hidden character a finding carries, so a
    // value read from a file reaches no terminal raw, quoted here or not.
    let coordinate = tool.coordinate();
    if entry.backend != coordinate {
        found.push(if entry.backend.is_empty() {
            format!("{label} installs through no backend, and {pins} pins {coordinate}")
        } else {
            format!(
                "{label} installs through {:?}, and {pins} pins {coordinate}",
                entry.backend
            )
        });
    }

    // Compared whole, so a tag, an asset or a byte a url parser drops can
    // differ in no way at all. No url is built from a version that is not one
    // exact release, which is refused where the pin file is read, and a
    // platform with no asset in `TOOLS` is refused where the platform list is.
    let expected = exact_release(version)
        .then(|| tool.url(&entry.platform, version))
        .flatten();
    match (&entry.url, expected) {
        (None, _) => found.push(format!("{label} records no url in {lock}")),
        (Some(url), Some(expected)) if *url != expected => found.push(format!(
            "{label} records the url {url:?}, and the release asset is {expected}"
        )),
        _ => {}
    }
    // Absence is refused here the same way a missing url is. mise writes this
    // field for every entry, so a lockfile without one has had it taken out,
    // and that is the quiet way to drop the reference an attestation lookup
    // reads. Its asset number binds no version, which the `url_replacements`
    // rule answers by keeping mise from ever fetching it.
    match &entry.url_api {
        None => found.push(format!("{label} records no url_api in {lock}")),
        Some(url_api) => {
            let prefix = tool.api_prefix();
            let numbered = url_api.strip_prefix(&prefix).is_some_and(|number| {
                !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
            });
            if !numbered {
                found.push(format!(
                    "{label} records the api reference {url_api:?}, and it is {prefix} and an asset number"
                ));
            }
        }
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
                format!("{label} records provenance {carried:?}, and no release of it is attested")
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
fn array_problems(pair: Pair, text: &str) -> Vec<String> {
    let lock = pair.lock;
    let counted = match locked_tools(pair, text) {
        Ok(counted) => counted,
        Err(problem) => return vec![problem],
    };
    let mut found = Vec::new();
    for tool in counted {
        if tool.entries > 1 {
            found.push(format!(
                "{lock} records {} entries for {}, so which one an install takes is undecided",
                tool.entries, tool.tool
            ));
        }
        if tool.platformless > 0 {
            found.push(format!(
                "{lock} records an entry for {} with no platform block, which no rule here reads",
                tool.tool
            ));
        }
        if tool.nested > 0 {
            found.push(format!(
                "{lock} records {}'s platforms as a nested table, which mise installs from and no rule here reads",
                tool.tool
            ));
        }
    }
    found
}

/// The directories below the repository root that mise reads configuration
/// from, whose every file the stray configuration rule reads.
const CONFIG_DIRS: &[&str] = &[".config", ".mise", "mise"];

/// One entry the stray configuration rule reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    /// The path from the repository root, with `/` separators.
    pub path: String,
    /// Whether the entry is a link, which is listed and never followed.
    pub link: bool,
}

/// What one directory entry is, read without following a link.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    File,
    Dir,
    Link,
}

/// Every entry at `root`, and every entry under the directories mise reads
/// configuration from, sorted by path.
///
/// A link is listed and never followed, so a linked directory reads as the one
/// entry it is. A directory's name matches without case, because Windows and
/// macOS open `.Config` when mise asks for `.config`.
///
/// # Errors
///
/// Returns the sentence a result row carries when a directory cannot be
/// listed.
pub fn config_paths(root: &Path) -> Result<Vec<TreeEntry>, String> {
    let mut found = Vec::new();
    for (name, kind) in listing(root)? {
        if kind == Kind::Dir && CONFIG_DIRS.contains(&name.to_ascii_lowercase().as_str()) {
            walk(&root.join(&name), &name, &mut found)?;
        }
        found.push(TreeEntry {
            path: name,
            link: kind == Kind::Link,
        });
    }
    found.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(found)
}

/// Every entry under `dir`, pushed as `prefix/<name>`.
fn walk(dir: &Path, prefix: &str, found: &mut Vec<TreeEntry>) -> Result<(), String> {
    for (name, kind) in listing(dir)? {
        let path = format!("{prefix}/{name}");
        if kind == Kind::Dir {
            walk(&dir.join(&name), &path, found)?;
        }
        found.push(TreeEntry {
            path,
            link: kind == Kind::Link,
        });
    }
    Ok(())
}

/// The entries of `dir`, each with what it is. A junction reads as a link on
/// Windows, the same as a symbolic link.
fn listing(dir: &Path) -> Result<Vec<(String, Kind)>, String> {
    let cannot = |err: std::io::Error| printable(format!("listing {}: {err}", dir.display()));
    fs::read_dir(dir)
        .map_err(cannot)?
        .map(|entry| {
            let entry = entry.map_err(cannot)?;
            let file_type = entry.file_type().map_err(cannot)?;
            let kind = if file_type.is_symlink() {
                Kind::Link
            } else if file_type.is_dir() {
                Kind::Dir
            } else {
                Kind::File
            };
            Ok((entry.file_name().to_string_lossy().into_owned(), kind))
        })
        .collect()
}

/// Whether mise reads `path` as configuration or as a lockfile beside it,
/// other than the two pairs the rules here hold.
///
/// Only the exact spellings of the pairs pass. Every other name is classified
/// without case, because Windows and macOS open `MISE.LOCAL.TOML` when mise
/// asks for `mise.local.toml`.
fn stray_config(path: &str) -> bool {
    if [PINS, LOCK, SEMVER.pins, SEMVER.lock].contains(&path) {
        return false;
    }
    let path = path.to_ascii_lowercase();
    match path.split_once('/') {
        Some((".mise" | "mise", _)) => true,
        Some((".config", rest)) => rest.starts_with("mise"),
        Some(_) => false,
        None => {
            let name = path.strip_prefix('.').unwrap_or(&path);
            matches!(name, "mise" | "miserc.toml" | "tool-versions")
                || matches!(
                    name.rsplit_once('.'),
                    Some((stem, "toml" | "lock")) if stem == "mise" || stem.starts_with("mise.")
                )
        }
    }
}

/// Every link, and every mise configuration or lockfile other than the two
/// pairs, among `entries`, as one sentence each.
///
/// mise merges every configuration file it discovers, and the lockfile beside
/// each, highest precedence first. A committed `mise.local.toml` and
/// `mise.local.lock` would decide what `mise install --locked` downloads while
/// [`PINS`] and [`LOCK`] still met every rule here, so any such file is
/// refused rather than read. mise follows a link where the listing does not,
/// so a link is refused whatever it names: a linked `.config` would carry
/// configuration past every name above. Neither repository tracks a link.
#[must_use]
pub fn stray_config_problems(entries: &[TreeEntry]) -> Vec<String> {
    entries
        .iter()
        .filter_map(|entry| {
            let path = &entry.path;
            if entry.link {
                Some(format!(
                    "{path:?} is a link, which mise would follow to configuration no rule here reads"
                ))
            } else if stray_config(path) {
                Some(format!(
                    "{path:?} is mise configuration beside {PINS}, which mise would merge over it"
                ))
            } else {
                None
            }
        })
        .map(printable)
        .collect()
}

/// Every way the pin files of [`PAIRS`] and the mise configurations in the
/// tree fall short, with an unreadable pin file reported as a problem of its
/// own.
///
/// `read` answers a file by its path from the root, and `paths` is the listing
/// [`config_paths`] makes. The gate's opening row and every mise start run
/// these same rules.
#[must_use]
pub fn file_problems(
    read: &dyn Fn(&str) -> Option<String>,
    paths: Result<Vec<TreeEntry>, String>,
) -> Vec<String> {
    let read = |path: &str| read(path).ok_or_else(|| format!("{path} cannot be read"));
    let mut found = match (
        read(MAIN.pins),
        read(MAIN.lock),
        read(SEMVER.pins),
        read(SEMVER.lock),
    ) {
        (Ok(pin_text), Ok(lock), Ok(semver_pins), Ok(semver_lock)) => {
            let mut found = problems(&pin_text, &lock);
            found.extend(semver_problems(&pin_text, &semver_pins, &semver_lock));
            found
        }
        (first, second, third, fourth) => [first, second, third, fourth]
            .into_iter()
            .filter_map(Result::err)
            .collect(),
    };
    match paths {
        Ok(paths) => found.extend(stray_config_problems(&paths)),
        Err(problem) => found.push(problem),
    }
    found
}

/// Every way the main pin files fall short, as one sentence each.
///
/// The two texts are passed in rather than read here, so the rules run the
/// same way against the repository and against a case written in a test.
#[must_use]
pub fn problems(pins: &str, lock: &str) -> Vec<String> {
    let mut found = match lockfile_platforms(pins) {
        Ok(platforms) => pair_problems(MAIN, &platforms, pins, lock),
        Err(problem) => vec![problem],
    };
    found.extend(replacement_problems(pins));
    found.extend(table_problems(pins));
    found.into_iter().map(printable).collect()
}

/// Every table [`PINS`] carries beyond [`PIN_TABLES`], and every way its
/// `[tool_config]` and `[settings]` differ from what the rules read.
///
/// mise runs hooks and tasks and exports an environment from this file, so
/// anything no rule reads could run beside every install. Both tables are
/// compared whole: a setting left out or turned off lifts a refusal mise
/// makes, and a setting added is one nothing here checks.
fn table_problems(pins: &str) -> Vec<String> {
    let Ok(document) = toml::from_str::<toml::Table>(pins) else {
        // A file that is not TOML is refused where the platform list is read.
        return Vec::new();
    };
    let mut found: Vec<String> = document
        .keys()
        .filter(|key| !PIN_TABLES.contains(&key.as_str()))
        .map(|key| {
            format!(
                "{PINS} carries {key:?}, and it holds [tools], [tool_config] and [settings] alone"
            )
        })
        .collect();
    found.extend(section_problems(&document, "tool_config", &["locked"], &[]));
    found.extend(section_problems(
        &document,
        "settings",
        SETTINGS_ON,
        SETTINGS_READ,
    ));
    if document.get("settings").is_some_and(toml::Value::is_table) {
        found.extend(section_problems(
            &document,
            "settings.aqua",
            AQUA_SETTINGS_ON,
            &[],
        ));
    }
    found
}

/// Every way the `[name]` table of [`PINS`] falls short of setting each of `on`
/// true and holding `read`, with nothing else beside them. `name` is the dotted
/// path of the table, as the findings name it.
fn section_problems(document: &toml::Table, name: &str, on: &[&str], read: &[&str]) -> Vec<String> {
    let found = name.split('.').try_fold(document, |table, segment| {
        table.get(segment).and_then(toml::Value::as_table)
    });
    let Some(table) = found else {
        return vec![format!("{PINS} holds no [{name}] table")];
    };
    let mut found: Vec<String> = table
        .keys()
        .filter(|key| !on.contains(&key.as_str()) && !read.contains(&key.as_str()))
        .map(|key| format!("{PINS} sets {key:?} under [{name}], which no rule here reads"))
        .collect();
    for key in on {
        if table.get(*key).and_then(toml::Value::as_bool) != Some(true) {
            found.push(format!("{PINS} does not set {key} = true under [{name}]"));
        }
    }
    found
}

/// Every option `file` gives the tool `name` beyond its version and the tag
/// prefix [`TOOLS`] holds for it.
///
/// mise honors a postinstall command, an install environment and an asset
/// pattern on a tool entry, and records the options in the lockfile beside
/// it, so both files hold an entry to the keys a rule here reads.
fn option_problems(file: &str, name: &str, options: &toml::Table) -> Vec<String> {
    let mut found = Vec::new();
    for (key, value) in options {
        match key.as_str() {
            "version" => {}
            "version_prefix" => {
                if let Some(tool) = tool_for(name)
                    && value.as_str() != Some(tool.tag_prefix)
                {
                    found.push(format!(
                        "{file} gives {name:?} the version_prefix {value}, and its release tags start {:?}",
                        tool.tag_prefix
                    ));
                }
            }
            _ => found.push(format!(
                "{file} gives {name:?} the option {key:?}, and a tool here carries its version and tag prefix alone"
            )),
        }
    }
    found
}

/// Every option a `[tools]` entry in `pair`'s pin file carries beyond the ones
/// [`option_problems`] allows.
fn pin_entry_problems(pair: Pair, pins: &str) -> Vec<String> {
    let Ok(document) = toml::from_str::<toml::Table>(pins) else {
        return Vec::new();
    };
    let Some(tools) = document.get("tools").and_then(toml::Value::as_table) else {
        return Vec::new();
    };
    tools
        .iter()
        .filter_map(|(name, value)| Some((name, value.as_table()?)))
        .flat_map(|(name, options)| option_problems(pair.pins, name, options))
        .collect()
}

/// Every key `pair`'s lockfile carries beyond the ones mise writes.
///
/// mise reads the options it records on an entry, and a key it does not write
/// is one no rule here reads either.
fn lock_key_problems(pair: Pair, text: &str) -> Vec<String> {
    let lock = pair.lock;
    let Ok(document) = toml::from_str::<toml::Table>(text) else {
        // A file that is not TOML is refused where its entries are read.
        return Vec::new();
    };
    let mut found: Vec<String> = document
        .keys()
        .filter(|key| !LOCK_KEYS.contains(&key.as_str()))
        .map(|key| format!("{lock} carries {key:?}, which mise does not write"))
        .collect();
    match document.get("lockfile_version") {
        Some(toml::Value::Integer(LOCKFILE_VERSION)) => {}
        Some(value) => found.push(format!(
            "{lock} gives lockfile_version {value}, and the rules here read version {LOCKFILE_VERSION} alone"
        )),
        None => found.push(format!(
            "{lock} carries no lockfile_version, and the rules here read version {LOCKFILE_VERSION} alone"
        )),
    }
    let Some(tools) = document.get("tools").and_then(toml::Value::as_table) else {
        return found;
    };
    for (name, entries) in tools {
        let tables = entries.as_array().into_iter().flatten();
        for entry in tables.filter_map(toml::Value::as_table) {
            for (key, value) in entry {
                if let Some(platform) = key.strip_prefix("platforms.") {
                    let fields = value.as_table().into_iter().flat_map(toml::Table::keys);
                    for field in fields.filter(|field| !PLATFORM_KEYS.contains(&field.as_str())) {
                        found.push(format!(
                            "{lock} gives {name:?} on {platform:?} the key {field:?}, which mise does not write"
                        ));
                    }
                } else if key == "options" {
                    match value.as_table() {
                        Some(options) => found.extend(option_problems(lock, name, options)),
                        None => found.push(format!(
                            "{lock} gives {name:?} options that are not a table"
                        )),
                    }
                } else if key != "platforms" && !ENTRY_KEYS.contains(&key.as_str()) {
                    // A nested `platforms` table is refused where entries are
                    // counted, so it is left to that rule here.
                    found.push(format!(
                        "{lock} gives {name:?} the key {key:?}, which mise does not write"
                    ));
                }
            }
        }
    }
    found
}

/// Whether [`PINS`] carries the one `url_replacements` rule, and nothing
/// beside it.
///
/// mise fetches a lockfile entry's `url_api` whenever a HEAD on its url fails,
/// and that reference names an asset by number alone. The rule sends every
/// such request to [`URL_API_REFUSED`], so the fallback fails rather than
/// installing another release. A second rule could redirect any download, so
/// the map is compared whole.
fn replacement_problems(pins: &str) -> Vec<String> {
    let Ok(document) = toml::from_str::<toml::Value>(pins) else {
        // A file that is not TOML is refused where the platform list is read.
        return Vec::new();
    };
    let rules = document
        .get("settings")
        .and_then(|settings| settings.get("url_replacements"))
        .and_then(toml::Value::as_table);
    let sound = rules.is_some_and(|rules| {
        rules.len() == 1
            && rules.get(URL_API_PATTERN).and_then(toml::Value::as_str) == Some(URL_API_REFUSED)
    });
    if sound {
        Vec::new()
    } else {
        vec![format!(
            "{PINS} carries url_replacements other than the one rule sending {URL_API_PATTERN} to {URL_API_REFUSED}"
        )]
    }
}

/// Every way the semver pin files fall short, as one sentence each.
///
/// The platforms come from `main_pins`, which mise loads beside this pair.
#[must_use]
pub fn semver_problems(main_pins: &str, pins: &str, lock: &str) -> Vec<String> {
    let mut found = match lockfile_platforms(main_pins) {
        Ok(platforms) => pair_problems(SEMVER, &platforms, pins, lock),
        Err(problem) => vec![problem],
    };
    // mise layers this file over mise.toml, and where both set a value this
    // one wins, so a [settings] or [tool_config] here could lift the locked
    // mode or the url_replacements rule mise.toml holds.
    if let Ok(document) = toml::from_str::<toml::Value>(pins)
        && let Some(table) = document.as_table()
    {
        for extra in table.keys().filter(|key| *key != "tools") {
            found.push(format!(
                "{} carries [{extra}], and it holds [tools] alone: everything else comes from {PINS}",
                SEMVER.pins
            ));
        }
    }
    found.into_iter().map(printable).collect()
}

/// Every way one pair falls short, for the platforms mise locks.
fn pair_problems(pair: Pair, platforms: &[String], pins: &str, lock: &str) -> Vec<String> {
    let (pin_file, lock_file) = (pair.pins, pair.lock);
    let mut found: Vec<String> = Vec::new();

    let tools = match pinned_tools(pair, pins) {
        Ok(tools) => tools,
        Err(problem) => return vec![problem],
    };
    if tools.is_empty() {
        return vec![format!("{pin_file} pins nothing")];
    }
    for (name, version) in &tools {
        if !exact_release(version) {
            found.push(format!(
                "{pin_file} pins {name} at {version:?}, which is not one exact release"
            ));
        }
        // A tool in the other pair's file loads where its pair does not: the
        // semver check in a job that holds a credential, which is what the
        // separate file exists to prevent.
        if let Some(tool) = tool_for(name)
            && tool.pair != pair
        {
            found.push(format!(
                "{pin_file} pins {name}, which belongs in {}",
                tool.pair.pins
            ));
        }
        // The url rule compares each entry against the asset named for its
        // platform, so a platform with none would go unchecked, and an asset
        // for an unlisted platform is a name nothing compares.
        if let Some(tool) = tool_for(name) {
            for platform in platforms {
                if !tool.assets.iter().any(|asset| asset.platform == platform) {
                    found.push(format!(
                        "{PINS} lists {platform} under lockfile_platforms, and no {platform} asset is named for {name}"
                    ));
                }
            }
            for asset in tool.assets {
                if !platforms.iter().any(|platform| platform == asset.platform) {
                    found.push(format!(
                        "{name} names a {0} asset, and {PINS} lists no {0} under lockfile_platforms",
                        asset.platform
                    ));
                }
            }
        }
    }
    if let Err(problem) = pinned_binaries(pair, pins) {
        found.push(problem);
    }
    let pinned: BTreeSet<&str> = tools.iter().map(|(name, _)| name.as_str()).collect();

    let locked_entries = match locked_platforms(pair, lock) {
        Ok(entries) => entries,
        Err(problem) => {
            found.push(problem);
            return found;
        }
    };

    found.extend(array_problems(pair, lock));
    found.extend(pin_entry_problems(pair, pins));
    found.extend(lock_key_problems(pair, lock));

    // Every platform block is read, not only the ones the list names. A block
    // nothing reads is a url and a checksum nobody checked, and a relock writes
    // exactly the listed platforms, so any other is an anomaly.
    for entry in &locked_entries {
        if pinned.contains(entry.tool.as_str()) && !platforms.contains(&entry.platform) {
            found.push(format!(
                "{lock_file} records a {} entry for {}, which is no platform the gate installs on",
                entry.platform, entry.tool
            ));
        }
    }

    for (name, version) in &tools {
        let Some(tool) = tool_for(name) else {
            continue;
        };
        for platform in platforms {
            if !locked_entries
                .iter()
                .any(|entry| &entry.tool == name && &entry.platform == platform)
            {
                found.push(format!("{name} records no {platform} entry in {lock_file}"));
            }
        }
        for entry in locked_entries.iter().filter(|entry| &entry.tool == name) {
            found.extend(entry_problems(pair, tool, version, entry));
        }
    }

    for name in locked_entries
        .iter()
        .map(|entry| &entry.tool)
        .collect::<BTreeSet<&String>>()
    {
        if !pinned.contains(name.as_str()) {
            found.push(format!(
                "{lock_file} locks {name}, which {pin_file} no longer pins"
            ));
        }
    }

    found.sort_unstable();
    found.dedup();
    found
}

#[cfg(test)]
mod tests {
    use super::{
        Backend, TOOLS, TreeEntry, config_paths, lockfile_platforms, problems, semver_problems,
        stray_config_problems,
    };
    use crate::tree::hidden;
    use std::collections::BTreeSet;
    use std::path::Path;

    /// A listed entry that is not a link.
    fn file(path: &str) -> TreeEntry {
        TreeEntry {
            path: path.to_string(),
            link: false,
        }
    }

    /// The platform list the sound pin file names.
    const PLATFORMS: &str = "[\"linux-x64\", \"macos-arm64\", \"windows-x64\"]";

    /// The one `url_replacements` rule the sound pin file carries, spelled as
    /// a reviewer reads it rather than built from the constant it has to equal.
    const RULE: &str = r#"url_replacements = { 'regex:^https://api\.github\.com/repos/[^/]+/[^/]+/releases/assets/.*$' = "https://url-api-refused.invalid/" }"#;

    /// A pin file every rule accepts, for the cases below to change one thing
    /// in.
    fn sound_pins() -> String {
        format!(
            "[tools]\ntaplo = \"0.10.0\"\n\"github:nextest-rs/nextest\" = {{ version = \"0.9.145\", version_prefix = \"cargo-nextest-\" }}\n\n[tool_config]\n{TOOL_CONFIG}\n\n[settings]\n{SOUND_SETTINGS}\nlockfile_platforms = {PLATFORMS}\n{RULE}\n\n[settings.aqua]\ngithub_attestations = true\n"
        )
    }

    /// The `[tool_config]` body the sound pin file carries.
    const TOOL_CONFIG: &str = "locked = true";

    /// The refusals the sound pin file turns on under `[settings]`.
    const SOUND_SETTINGS: &str = "locked = true\nlockfile = true\nlocked_verify_provenance = true\nprovenance_api_failures_fatal = true\ngithub_attestations = true";

    /// Two digests of the right shape, for the cases that name one.
    const DIGEST_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const DIGEST_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    /// A lockfile every rule accepts, shaped as mise writes one.
    ///
    /// taplo carries no provenance and nextest carries one, so the sound
    /// document holds both halves of that rule.
    const SOUND_LOCK: &str = concat!(
        "lockfile_version = 1\n\n",
        "[[tools.taplo]]\nversion = \"0.10.0\"\nbackend = \"aqua:tamasfe/taplo\"\n",
        "[tools.taplo.\"platforms.linux-x64\"]\nchecksum = \"sha256:",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"\n",
        "url = \"https://github.com/tamasfe/taplo/releases/download/0.10.0/taplo-linux-x86_64.gz\"\n",
        "url_api = \"https://api.github.com/repos/tamasfe/taplo/releases/assets/257322600\"\n",
        "[tools.taplo.\"platforms.macos-arm64\"]\nchecksum = \"sha256:",
        "9999999999999999999999999999999999999999999999999999999999999999\"\n",
        "url = \"https://github.com/tamasfe/taplo/releases/download/0.10.0/taplo-darwin-aarch64.gz\"\n",
        "url_api = \"https://api.github.com/repos/tamasfe/taplo/releases/assets/257322740\"\n",
        "[tools.taplo.\"platforms.windows-x64\"]\nchecksum = \"sha256:",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\"\n",
        "url = \"https://github.com/tamasfe/taplo/releases/download/0.10.0/taplo-windows-x86_64.zip\"\n",
        "url_api = \"https://api.github.com/repos/tamasfe/taplo/releases/assets/257323062\"\n",
        "[[tools.\"github:nextest-rs/nextest\"]]\nversion = \"0.9.145\"\n",
        "backend = \"github:nextest-rs/nextest\"\nspecifiers = [\"0.9.145\"]\n",
        "[tools.\"github:nextest-rs/nextest\".options]\nversion_prefix = \"cargo-nextest-\"\n",
        "[tools.\"github:nextest-rs/nextest\".\"platforms.linux-x64\"]\nchecksum = \"sha256:",
        "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc\"\n",
        "url = \"https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-0.9.145/cargo-nextest-0.9.145-x86_64-unknown-linux-gnu.tar.gz\"\n",
        "url_api = \"https://api.github.com/repos/nextest-rs/nextest/releases/assets/568838794\"\n",
        "provenance = \"github-attestations\"\n",
        "[tools.\"github:nextest-rs/nextest\".\"platforms.macos-arm64\"]\nchecksum = \"sha256:",
        "8888888888888888888888888888888888888888888888888888888888888888\"\n",
        "url = \"https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-0.9.145/cargo-nextest-0.9.145-universal-apple-darwin.tar.gz\"\n",
        "url_api = \"https://api.github.com/repos/nextest-rs/nextest/releases/assets/568841216\"\n",
        "provenance = \"github-attestations\"\n",
        "[tools.\"github:nextest-rs/nextest\".\"platforms.windows-x64\"]\nchecksum = \"sha256:",
        "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd\"\n",
        "url = \"https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-0.9.145/cargo-nextest-0.9.145-x86_64-pc-windows-msvc.zip\"\n",
        "url_api = \"https://api.github.com/repos/nextest-rs/nextest/releases/assets/568848390\"\n",
        "provenance = \"github-attestations\"\n",
    );

    /// The sound documents pass, so every refusal below changes one thing from
    /// something that worked.
    #[test]
    fn the_sound_documents_meet_every_rule() {
        assert_eq!(problems(&sound_pins(), SOUND_LOCK), Vec::<String>::new());
    }

    /// Run each case and assert the rules said the thing it names.
    ///
    /// A case is the sound pair with one thing changed, so a rule that goes
    /// quiet shows up as the sentence nobody produced.
    fn refuses(cases: &[(&str, String, String, &str)]) {
        for (what, pins, lock, wanted) in cases {
            let found = problems(pins, lock);
            assert!(
                found.iter().any(|problem| problem.contains(wanted)),
                "{what}: nothing said {wanted:?}, got {found:?}"
            );
        }
    }

    /// The rules refuse each shape they name, against documents written here.
    ///
    /// A rule nothing can break is a rule that proves nothing, so every case is
    /// the sound pair with one thing changed. The url and backend cases are the
    /// ones a checksum rule alone accepts: each leaves the lockfile agreeing
    /// with itself and disagreeing with `TOOLS`.
    #[test]
    fn the_pin_rules_refuse_the_shapes_they_name() {
        let cases: &[(&str, String, String, &str)] = &[
            (
                "the platform list deleted",
                sound_pins().replace(&format!("lockfile_platforms = {PLATFORMS}\n"), ""),
                SOUND_LOCK.to_string(),
                "names no lockfile_platforms under [settings]",
            ),
            (
                "the platform list emptied",
                sound_pins().replace(PLATFORMS, "[]"),
                SOUND_LOCK.to_string(),
                "names no lockfile_platforms under [settings]",
            ),
            (
                "a platform dropped from the list, its blocks kept",
                sound_pins().replace(PLATFORMS, "[\"linux-x64\", \"macos-arm64\"]"),
                SOUND_LOCK.to_string(),
                "records a windows-x64 entry for taplo, which is no platform the gate installs on",
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
        let cases: &[(&str, String, String, &str)] = &[
            (
                "an attested tool's provenance line deleted",
                sound_pins(),
                SOUND_LOCK.replace("provenance = \"github-attestations\"\n", ""),
                "records no provenance, and github-attestations is required for it",
            ),
            (
                "a provenance line added to a tool with no attestation",
                sound_pins(),
                SOUND_LOCK.replace(
                    "url_api = \"https://api.github.com/repos/tamasfe/taplo/releases/assets/257322600\"\n",
                    "url_api = \"https://api.github.com/repos/tamasfe/taplo/releases/assets/257322600\"\nprovenance = \"github-attestations\"\n",
                ),
                "records provenance \"github-attestations\", and no release of it is attested",
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
        let cases: &[(&str, String, String, &str)] = &[
            (
                "a second entry for a tool, beside the sound one",
                sound_pins(),
                format!(
                    "{SOUND_LOCK}[[tools.taplo]]\nversion = \"0.10.0\"\nbackend = \"aqua:attacker/taplo\"\n[tools.taplo.\"platforms.linux-x64\"]\nchecksum = \"sha256:{DIGEST_A}\"\nurl = \"https://cdn.attacker.example/taplo/payload.gz\"\n"
                ),
                "records 2 entries for taplo",
            ),
            (
                "a second entry carrying no platform block",
                sound_pins(),
                format!(
                    "{SOUND_LOCK}[[tools.taplo]]\nversion = \"0.10.0\"\nbackend = \"aqua:attacker/taplo\"\n"
                ),
                "records an entry for taplo with no platform block",
            ),
            (
                "a second entry carrying only an options table",
                sound_pins(),
                format!(
                    "{SOUND_LOCK}[[tools.taplo]]\nversion = \"9.9.9\"\nbackend = \"aqua:attacker/taplo\"\n[tools.taplo.options]\nversion_prefix = \"attacker-\"\n"
                ),
                "records an entry for taplo with no platform block",
            ),
            (
                "a platform block the gate never installs on",
                sound_pins(),
                format!(
                    "{SOUND_LOCK}[tools.taplo.\"platforms.linux-arm64\"]\nchecksum = \"md5:00\"\nurl = \"https://cdn.attacker.example/taplo/payload.gz\"\n"
                ),
                "records a linux-arm64 entry for taplo, which is no platform the gate installs on",
            ),
            (
                "a truncated digest",
                sound_pins(),
                SOUND_LOCK.replace(&format!("sha256:{DIGEST_A}"), "sha256:0"),
                "and a sha256 digest is 64 hex digits",
            ),
            (
                "a checksum dropped, the platform block kept",
                sound_pins(),
                SOUND_LOCK.replace(&format!("checksum = \"sha256:{DIGEST_B}\"\n"), ""),
                "taplo on windows-x64 carries no checksum",
            ),
            (
                "a platform block dropped",
                sound_pins(),
                SOUND_LOCK.replace(
                    &format!(
                        "[tools.taplo.\"platforms.linux-x64\"]\nchecksum = \"sha256:{DIGEST_A}\"\n"
                    ),
                    "",
                ),
                "taplo records no linux-x64 entry",
            ),
            (
                "the two files disagreeing on a version",
                sound_pins().replace("taplo = \"0.10.0\"", "taplo = \"0.10.1\""),
                SOUND_LOCK.to_string(),
                "pins taplo \"0.10.1\" and mise.lock records \"0.10.0\"",
            ),
            (
                "a range in place of an exact release",
                sound_pins().replace("taplo = \"0.10.0\"", "taplo = \"0.10\""),
                SOUND_LOCK.to_string(),
                "which is not one exact release",
            ),
            (
                "a lockfile entry the pin file no longer names",
                sound_pins().replace("taplo = \"0.10.0\"\n", ""),
                SOUND_LOCK.to_string(),
                "locks taplo, which mise.toml no longer pins",
            ),
            (
                "a key naming no binary",
                sound_pins().replace("github:nextest-rs/nextest", "github:nextest-rs/elsewhere"),
                SOUND_LOCK.replace("github:nextest-rs/nextest", "github:nextest-rs/elsewhere"),
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
        let cases: &[(&str, String, String, &str)] = &[
            (
                "the backend moved to another account",
                sound_pins(),
                SOUND_LOCK.replace("aqua:tamasfe/taplo", "aqua:attacker/taplo"),
                "installs through \"aqua:attacker/taplo\", and mise.toml pins aqua:tamasfe/taplo",
            ),
            (
                "the backend switched to the other kind",
                sound_pins(),
                SOUND_LOCK.replace("aqua:tamasfe/taplo", "github:tamasfe/taplo"),
                "installs through \"github:tamasfe/taplo\"",
            ),
            (
                "the backend line dropped",
                sound_pins(),
                SOUND_LOCK.replace("backend = \"aqua:tamasfe/taplo\"\n", ""),
                "installs through no backend",
            ),
        ];
        refuses(cases);
    }

    /// The sound linux-x64 taplo url, which every url case below changes.
    const TAPLO_LINUX: &str =
        "https://github.com/tamasfe/taplo/releases/download/0.10.0/taplo-linux-x86_64.gz";

    /// The sentence the url rule says for the linux-x64 taplo entry, whatever
    /// the url it records.
    const TAPLO_LINUX_REFUSED: &str = ", and the release asset is https://github.com/tamasfe/taplo/releases/download/0.10.0/taplo-linux-x86_64.gz";

    /// The rules refuse a url that moved to another host, owner, release or
    /// asset, which a checksum rule alone accepts.
    ///
    /// Each case leaves the lockfile agreeing with itself and disagreeing with
    /// the url built from `TOOLS`.
    #[test]
    fn the_lockfile_rules_refuse_a_moved_url() {
        let moved = |to: &str| SOUND_LOCK.replace(TAPLO_LINUX, to);
        let cases: &[(&str, String, String, &str)] = &[
            (
                "another host",
                sound_pins(),
                moved("https://cdn.attacker.example/tamasfe/taplo/releases/download/0.10.0/taplo-linux-x86_64.gz"),
                TAPLO_LINUX_REFUSED,
            ),
            (
                "a host whose text holds the real one",
                sound_pins(),
                moved("https://github.com.attacker.example/tamasfe/taplo/releases/download/0.10.0/taplo-linux-x86_64.gz"),
                TAPLO_LINUX_REFUSED,
            ),
            (
                "another owner",
                sound_pins(),
                moved("https://github.com/attacker/taplo/releases/download/0.10.0/taplo-linux-x86_64.gz"),
                TAPLO_LINUX_REFUSED,
            ),
            (
                "an older release of the genuine repository",
                sound_pins(),
                moved("https://github.com/tamasfe/taplo/releases/download/0.9.3/taplo-linux-x86_64.gz"),
                TAPLO_LINUX_REFUSED,
            ),
            (
                "an older tag with the pinned release moved into the filename",
                sound_pins(),
                moved("https://github.com/tamasfe/taplo/releases/download/0.9.3/taplo-0.10.0-linux-x86_64.gz"),
                TAPLO_LINUX_REFUSED,
            ),
            (
                "a prerelease tag that starts with the pinned release",
                sound_pins(),
                moved("https://github.com/tamasfe/taplo/releases/download/0.10.0-rc1/taplo-linux-x86_64.gz"),
                TAPLO_LINUX_REFUSED,
            ),
            (
                "another platform's asset",
                sound_pins(),
                SOUND_LOCK.replace(
                    "https://github.com/tamasfe/taplo/releases/download/0.10.0/taplo-windows-x86_64.zip",
                    TAPLO_LINUX,
                ),
                ", and the release asset is https://github.com/tamasfe/taplo/releases/download/0.10.0/taplo-windows-x86_64.zip",
            ),
            (
                "the url dropped",
                sound_pins(),
                SOUND_LOCK.replace(&format!("url = \"{TAPLO_LINUX}\"\n"), ""),
                "taplo on linux-x64 records no url",
            ),
        ];
        refuses(cases);
    }

    /// The rules refuse a url some parser reads as the genuine one, and one
    /// GitHub answers with a 404, which sends mise to the api reference.
    ///
    /// Equality refuses each with no parser to disagree with mise's.
    #[test]
    fn the_lockfile_rules_refuse_a_url_a_parser_reads_as_genuine() {
        let urls: &[(&str, &str)] = &[
            (
                "a walk back out of the release path",
                "https://github.com/tamasfe/taplo/releases/download/0.10.0/../../../../koalaman/shellcheck/releases/download/v0.11.0/shellcheck-v0.11.0.linux.x86_64.tar.xz",
            ),
            (
                "plain http",
                "http://github.com/tamasfe/taplo/releases/download/0.10.0/taplo-linux-x86_64.gz",
            ),
            (
                "userinfo ahead of the real host",
                "https://github.com@attacker.example/tamasfe/taplo/releases/download/0.10.0/taplo-linux-x86_64.gz",
            ),
            (
                "a port",
                "https://github.com:8443/tamasfe/taplo/releases/download/0.10.0/taplo-linux-x86_64.gz",
            ),
            (
                "a query",
                "https://github.com/tamasfe/taplo/releases/download/0.10.0/taplo-linux-x86_64.gz?x=1",
            ),
            (
                "a fragment",
                "https://github.com/tamasfe/taplo/releases/download/0.10.0/taplo-linux-x86_64.gz#x",
            ),
            (
                "a percent escape walking out of the release",
                "https://github.com/tamasfe/taplo/releases/download/0.10.0/%2e%2e/taplo-linux-x86_64.gz",
            ),
            (
                "a percent escape in the host",
                "https://%67ithub.com/tamasfe/taplo/releases/download/0.10.0/taplo-linux-x86_64.gz",
            ),
            (
                "a backslash",
                "https://github.com\\\\attacker.example/tamasfe/taplo/releases/download/0.10.0/taplo-linux-x86_64.gz",
            ),
            (
                "a tab a url parser drops",
                "https://github.com/tamasfe/taplo/releases/download/0.10.0/\\ttaplo-linux-x86_64.gz",
            ),
            (
                "the host in capitals",
                "https://GitHub.COM/tamasfe/taplo/releases/download/0.10.0/taplo-linux-x86_64.gz",
            ),
            (
                "an extra segment GitHub answers with a 404",
                "https://github.com/tamasfe/taplo/releases/download/0.10.0/extra/taplo-linux-x86_64.gz",
            ),
            (
                "a missing asset GitHub answers with a 404",
                "https://github.com/tamasfe/taplo/releases/download/0.10.0/taplo-missing.gz",
            ),
        ];
        let cases: Vec<(&str, String, String, &str)> = urls
            .iter()
            .map(|(what, url)| {
                (
                    *what,
                    sound_pins(),
                    SOUND_LOCK.replace(TAPLO_LINUX, url),
                    TAPLO_LINUX_REFUSED,
                )
            })
            .collect();
        refuses(&cases);
    }

    /// A version that walks out of the release, pinned and locked alike.
    const WALK: &str = "../../../../koalaman/shellcheck/releases/download/v0.11.0";

    /// The rules refuse a version that is not one exact release, before any
    /// url is built from it.
    ///
    /// The version is written into the url an entry is compared against, so a
    /// walking version pinned and locked alike yields a lockfile url equal to
    /// the one built, and only the version rule stands in its way.
    #[test]
    fn a_version_walking_out_of_the_release_is_refused() {
        refuses(&[(
            "a walking version in the pin file and the lockfile",
            sound_pins().replace("taplo = \"0.10.0\"", &format!("taplo = \"{WALK}\"")),
            SOUND_LOCK.replace("0.10.0", WALK),
            "pins taplo at \"../../../../koalaman/shellcheck/releases/download/v0.11.0\", which is not one exact release",
        )]);
    }

    /// The sentence the api rule says for any taplo entry.
    const TAPLO_API_REFUSED: &str =
        "and it is https://api.github.com/repos/tamasfe/taplo/releases/assets/ and an asset number";

    /// The rules refuse any api reference but taplo's prefix and a number.
    ///
    /// The asset number itself binds nothing offline, which the
    /// `url_replacements` rule answers. What is checked here is everything
    /// around it: GitHub answers a number under another repository's path with
    /// a 404, so the prefix binds the repository.
    #[test]
    fn the_lockfile_rules_refuse_an_api_reference_off_the_repository() {
        let moved = |to: &str| {
            SOUND_LOCK.replace(
                "https://api.github.com/repos/tamasfe/taplo/releases/assets/257322600",
                to,
            )
        };
        let cases: &[(&str, String, String, &str)] = &[
            (
                "another host",
                sound_pins(),
                moved("https://api.github.com.attacker.example/repos/tamasfe/taplo/releases/assets/257322600"),
                TAPLO_API_REFUSED,
            ),
            (
                "another owner",
                sound_pins(),
                moved("https://api.github.com/repos/koalaman/shellcheck/releases/assets/279056944"),
                TAPLO_API_REFUSED,
            ),
            (
                "a tail that is not a number",
                sound_pins(),
                moved("https://api.github.com/repos/tamasfe/taplo/releases/assets/257322600x"),
                TAPLO_API_REFUSED,
            ),
            (
                "no number at all",
                sound_pins(),
                moved("https://api.github.com/repos/tamasfe/taplo/releases/assets/"),
                TAPLO_API_REFUSED,
            ),
            (
                "an extra segment",
                sound_pins(),
                moved("https://api.github.com/repos/tamasfe/taplo/releases/assets/257322600/1"),
                TAPLO_API_REFUSED,
            ),
            (
                "a walk to another repository",
                sound_pins(),
                moved("https://api.github.com/repos/tamasfe/taplo/releases/assets/../../../../koalaman/shellcheck/releases/assets/279056944"),
                TAPLO_API_REFUSED,
            ),
            (
                "the api reference dropped",
                sound_pins(),
                SOUND_LOCK.replace(
                    "url_api = \"https://api.github.com/repos/tamasfe/taplo/releases/assets/257322600\"\n",
                    "",
                ),
                "records no url_api",
            ),
        ];
        refuses(cases);
    }

    /// The rules refuse a platform list and an asset table that disagree.
    ///
    /// A listed platform with no asset would leave its url compared against
    /// nothing, and an asset for an unlisted platform is a name no url is
    /// compared with.
    #[test]
    fn the_asset_table_covers_the_platform_list_exactly() {
        let cases: &[(&str, String, String, &str)] = &[
            (
                "a listed platform no asset is named for",
                sound_pins().replace(
                    PLATFORMS,
                    "[\"linux-arm64\", \"linux-x64\", \"macos-arm64\", \"windows-x64\"]",
                ),
                SOUND_LOCK.to_string(),
                "mise.toml lists linux-arm64 under lockfile_platforms, and no linux-arm64 asset is named for taplo",
            ),
            (
                "an asset for a platform the list drops",
                sound_pins().replace(PLATFORMS, "[\"linux-x64\", \"windows-x64\"]"),
                SOUND_LOCK.to_string(),
                "taplo names a macos-arm64 asset, and mise.toml lists no macos-arm64 under lockfile_platforms",
            ),
        ];
        refuses(cases);
    }

    /// The rules refuse a nested `platforms` table.
    ///
    /// mise installs from the nested form for any platform, and the per-entry
    /// rules read only the quoted `platforms.<name>` keys, so a nested table
    /// is a url and a checksum nothing here would compare.
    #[test]
    fn the_lockfile_rules_refuse_a_nested_platform_table() {
        let nested = |platform: &str| {
            SOUND_LOCK.replace(
                "[[tools.\"github:nextest-rs/nextest\"]]",
                &format!(
                    "[tools.taplo.platforms.{platform}]\nchecksum = \"sha256:{DIGEST_A}\"\nurl = \"https://github.com/koalaman/shellcheck/releases/download/v0.11.0/shellcheck-v0.11.0.zip\"\n[[tools.\"github:nextest-rs/nextest\"]]"
                ),
            )
        };
        let cases: &[(&str, String, String, &str)] = &[
            (
                "a nested table for a platform the list leaves out",
                sound_pins(),
                nested("linux-arm64"),
                "records taplo's platforms as a nested table",
            ),
            (
                "a nested table beside the quoted one for a listed platform",
                sound_pins(),
                nested("windows-x64"),
                "records taplo's platforms as a nested table",
            ),
        ];
        refuses(cases);
    }

    /// The sentence the `url_replacements` rule says.
    const RULE_REFUSED: &str = "mise.toml carries url_replacements other than the one rule";

    /// The rules refuse any `url_replacements` map but the one rule.
    ///
    /// Without it, a failed HEAD on a url sends mise to the api reference,
    /// whose asset number can name another release. A second entry could
    /// redirect any download, so the map is compared whole.
    #[test]
    fn the_pin_rules_hold_the_one_url_replacement() {
        let cases: &[(&str, String, String, &str)] = &[
            (
                "the rule deleted",
                sound_pins().replace(&format!("{RULE}\n"), ""),
                SOUND_LOCK.to_string(),
                RULE_REFUSED,
            ),
            (
                "the map emptied",
                sound_pins().replace(RULE, "url_replacements = {}"),
                SOUND_LOCK.to_string(),
                RULE_REFUSED,
            ),
            (
                "the target moved to a host that resolves",
                sound_pins().replace("https://url-api-refused.invalid/", "https://mirror.attacker.example/"),
                SOUND_LOCK.to_string(),
                RULE_REFUSED,
            ),
            (
                "the pattern narrowed to one repository",
                sound_pins().replace("repos/[^/]+/[^/]+/releases", "repos/tamasfe/taplo/releases"),
                SOUND_LOCK.to_string(),
                RULE_REFUSED,
            ),
            (
                "a second entry beside the rule",
                sound_pins().replace(
                    " = \"https://url-api-refused.invalid/\" }",
                    " = \"https://url-api-refused.invalid/\", 'https://github.com/' = \"https://mirror.attacker.example/\" }",
                ),
                SOUND_LOCK.to_string(),
                RULE_REFUSED,
            ),
            (
                "the map written as a string",
                sound_pins().replace(RULE, "url_replacements = \"https://url-api-refused.invalid/\""),
                SOUND_LOCK.to_string(),
                RULE_REFUSED,
            ),
        ];
        refuses(cases);
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
        assert_eq!(
            aqua.url("linux-x64", "0.10.0").as_deref(),
            Some("https://github.com/tamasfe/taplo/releases/download/0.10.0/taplo-linux-x86_64.gz")
        );
        assert_eq!(aqua.url("linux-arm64", "0.10.0"), None);
        assert_eq!(
            aqua.api_prefix(),
            "https://api.github.com/repos/tamasfe/taplo/releases/assets/"
        );

        let github = TOOLS
            .iter()
            .find(|tool| tool.key == "github:bnjbvr/cargo-machete")
            .expect("cargo-machete is pinned");
        assert_eq!(github.coordinate(), "github:bnjbvr/cargo-machete");
        assert_eq!(github.backend, Backend::Github);
        // The version fills the tag and the asset name alike.
        assert_eq!(
            github.url("windows-x64", "0.9.2").as_deref(),
            Some(
                "https://github.com/bnjbvr/cargo-machete/releases/download/v0.9.2/cargo-machete-v0.9.2-x86_64-pc-windows-msvc.tar.gz"
            )
        );
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

    /// A semver pin file every rule accepts beside [`sound_pins`].
    const SOUND_SEMVER_PINS: &str =
        "[tools]\n\"github:obi1kenobi/cargo-semver-checks\" = \"0.50.0\"\n";

    /// Its lockfile, for the platforms [`sound_pins`] names, shaped as mise
    /// writes one.
    const SOUND_SEMVER_LOCK: &str = concat!(
        "lockfile_version = 1\n\n",
        "[[tools.\"github:obi1kenobi/cargo-semver-checks\"]]\nversion = \"0.50.0\"\n",
        "backend = \"github:obi1kenobi/cargo-semver-checks\"\n",
        "[tools.\"github:obi1kenobi/cargo-semver-checks\".\"platforms.linux-x64\"]\n",
        "checksum = \"sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee\"\n",
        "url = \"https://github.com/obi1kenobi/cargo-semver-checks/releases/download/v0.50.0/cargo-semver-checks-x86_64-unknown-linux-gnu.tar.gz\"\n",
        "url_api = \"https://api.github.com/repos/obi1kenobi/cargo-semver-checks/releases/assets/498085744\"\n",
        "[tools.\"github:obi1kenobi/cargo-semver-checks\".\"platforms.macos-arm64\"]\n",
        "checksum = \"sha256:7777777777777777777777777777777777777777777777777777777777777777\"\n",
        "url = \"https://github.com/obi1kenobi/cargo-semver-checks/releases/download/v0.50.0/cargo-semver-checks-aarch64-apple-darwin.tar.gz\"\n",
        "url_api = \"https://api.github.com/repos/obi1kenobi/cargo-semver-checks/releases/assets/498087310\"\n",
        "[tools.\"github:obi1kenobi/cargo-semver-checks\".\"platforms.windows-x64\"]\n",
        "checksum = \"sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff\"\n",
        "url = \"https://github.com/obi1kenobi/cargo-semver-checks/releases/download/v0.50.0/cargo-semver-checks-x86_64-pc-windows-msvc.zip\"\n",
        "url_api = \"https://api.github.com/repos/obi1kenobi/cargo-semver-checks/releases/assets/498090877\"\n",
    );

    /// The semver pair passes beside the main one, so every refusal below
    /// changes one thing from something that worked.
    #[test]
    fn the_sound_semver_pair_meets_every_rule() {
        assert_eq!(
            semver_problems(&sound_pins(), SOUND_SEMVER_PINS, SOUND_SEMVER_LOCK),
            Vec::<String>::new()
        );
    }

    /// Run each semver case and assert the rules said the thing it names.
    fn semver_refuses(cases: &[(&str, String, String, String, &str)]) {
        for (what, main_pins, pins, lock, wanted) in cases {
            let found = semver_problems(main_pins, pins, lock);
            assert!(
                found.iter().any(|problem| problem.contains(wanted)),
                "{what}: nothing said {wanted:?}, got {found:?}"
            );
        }
    }

    /// A tool pinned in the other pair's file is refused either way round.
    ///
    /// cargo-semver-checks in mise.toml is the case the separate file exists
    /// for: every job loads mise.toml, so a job holding the releaser token could
    /// install it and run the check.
    #[test]
    fn a_tool_in_the_other_pairs_file_is_refused() {
        let semver_in_main_pins = sound_pins().replace(
            "[tools]\n",
            "[tools]\n\"github:obi1kenobi/cargo-semver-checks\" = \"0.50.0\"\n",
        );
        let semver_in_main_lock = format!("{SOUND_LOCK}{SOUND_SEMVER_LOCK}");
        let found = problems(&semver_in_main_pins, &semver_in_main_lock);
        assert!(
            found.iter().any(|problem| problem.contains(
                "mise.toml pins github:obi1kenobi/cargo-semver-checks, which belongs in mise.semver.toml"
            )),
            "cargo-semver-checks in mise.toml: got {found:?}"
        );

        semver_refuses(&[(
            "taplo in the semver file",
            sound_pins(),
            format!("{SOUND_SEMVER_PINS}taplo = \"0.10.0\"\n"),
            SOUND_SEMVER_LOCK.to_string(),
            "mise.semver.toml pins taplo, which belongs in mise.toml",
        )]);
    }

    /// The semver rules name the semver files and read their platforms from
    /// mise.toml.
    ///
    /// Each case would pass unnoticed if the sentence named the main files: a
    /// reader relocking mise.lock for a problem in mise.semver.lock fixes
    /// nothing.
    #[test]
    fn the_semver_rules_name_the_semver_files() {
        let main = || sound_pins();
        let pins = || SOUND_SEMVER_PINS.to_string();
        let lock = || SOUND_SEMVER_LOCK.to_string();
        semver_refuses(&[
            (
                "a checksum dropped",
                main(),
                pins(),
                SOUND_SEMVER_LOCK.replace(
                    "checksum = \"sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff\"\n",
                    "",
                ),
                "carries no checksum in mise.semver.lock",
            ),
            (
                "a platform block dropped",
                main(),
                pins(),
                SOUND_SEMVER_LOCK.replace(
                    "[tools.\"github:obi1kenobi/cargo-semver-checks\".\"platforms.linux-x64\"]\n",
                    "",
                ),
                "records no linux-x64 entry in mise.semver.lock",
            ),
            (
                "the two files disagreeing on a version",
                main(),
                SOUND_SEMVER_PINS.replace("0.50.0", "0.50.1"),
                lock(),
                "mise.semver.toml pins github:obi1kenobi/cargo-semver-checks \"0.50.1\" and mise.semver.lock records \"0.50.0\"",
            ),
            (
                "the url naming another release",
                main(),
                pins(),
                SOUND_SEMVER_LOCK.replace(
                    "download/v0.50.0/cargo-semver-checks-x86_64-unknown",
                    "download/v0.49.0/cargo-semver-checks-x86_64-unknown",
                ),
                ", and the release asset is https://github.com/obi1kenobi/cargo-semver-checks/releases/download/v0.50.0/cargo-semver-checks-x86_64-unknown-linux-gnu.tar.gz",
            ),
            (
                "the api reference under another repository",
                main(),
                pins(),
                SOUND_SEMVER_LOCK.replace(
                    "repos/obi1kenobi/cargo-semver-checks/releases/assets/498085744",
                    "repos/obi1kenobi/elsewhere/releases/assets/498085744",
                ),
                "and it is https://api.github.com/repos/obi1kenobi/cargo-semver-checks/releases/assets/ and an asset number",
            ),
            (
                "a settings table that would lift the rules mise.toml holds",
                main(),
                format!("{SOUND_SEMVER_PINS}\n[settings]\nurl_replacements = {{}}\n"),
                lock(),
                "mise.semver.toml carries [settings], and it holds [tools] alone",
            ),
            (
                "a tool_config table that would lift the locked mode",
                main(),
                format!("{SOUND_SEMVER_PINS}\n[tool_config]\nlocked = false\n"),
                lock(),
                "mise.semver.toml carries [tool_config], and it holds [tools] alone",
            ),
            (
                "a lockfile entry the semver file no longer names",
                main(),
                pins(),
                format!("{SOUND_SEMVER_LOCK}{SOUND_LOCK}"),
                "mise.semver.lock locks taplo, which mise.semver.toml no longer pins",
            ),
            (
                "the semver file pinning nothing",
                main(),
                "[tools]\n".to_string(),
                lock(),
                "mise.semver.toml pins nothing",
            ),
            (
                "a key naming no binary",
                main(),
                SOUND_SEMVER_PINS.replace(
                    "github:obi1kenobi/cargo-semver-checks",
                    "github:obi1kenobi/elsewhere",
                ),
                SOUND_SEMVER_LOCK.replace(
                    "github:obi1kenobi/cargo-semver-checks",
                    "github:obi1kenobi/elsewhere",
                ),
                "\"github:obi1kenobi/elsewhere\" is a key no entry names a binary and a release for",
            ),
            (
                "a platform mise.toml no longer lists",
                sound_pins().replace(PLATFORMS, "[\"linux-x64\", \"macos-arm64\"]"),
                pins(),
                lock(),
                "mise.semver.lock records a windows-x64 entry for github:obi1kenobi/cargo-semver-checks, which is no platform the gate installs on",
            ),
        ]);
    }

    /// The platform list is read from the settings table alone, in order.
    #[test]
    fn the_platform_list_is_read_from_the_settings_table() {
        let listed = lockfile_platforms(&sound_pins()).expect("the sound pins name platforms");
        assert_eq!(listed, ["linux-x64", "macos-arm64", "windows-x64"]);
        assert!(lockfile_platforms("[tools]\na = \"1.0.0\"\n").is_err());
        assert!(lockfile_platforms("not toml = = =").is_err());
        assert!(
            lockfile_platforms("[settings]\nlockfile_platforms = [\"linux-x64\", 1]\n")
                .is_err_and(|problem| problem.contains("entry 1"))
        );
    }

    /// Every other mise configuration or lockfile is refused, and the two
    /// pairs and files mise never reads are not.
    ///
    /// mise merges a discovered configuration file and the lockfile beside it
    /// over `mise.toml` and `mise.lock`, so each refused path here is one a
    /// committed file could use to decide what an install downloads.
    #[test]
    fn every_other_mise_configuration_is_refused() {
        for path in [
            "mise.local.toml",
            "mise.local.lock",
            ".mise.toml",
            ".mise.local.toml",
            "mise.ci.toml",
            "mise.ci.lock",
            ".mise.ci.toml",
            ".miserc.toml",
            ".tool-versions",
            ".mise",
            "mise",
            ".config/mise.toml",
            ".config/mise.lock",
            ".config/mise",
            ".config/mise/config.toml",
            ".config/mise/conf.d/extra.toml",
            ".config/miserc.toml",
            ".mise/config.toml",
            "mise/config.toml",
            "MISE.LOCAL.TOML",
            "Mise.Local.Lock",
            ".MISE.toml",
            "MISE.TOML",
            "Mise.Semver.Lock",
            ".Tool-Versions",
            ".CONFIG/mise.toml",
            ".config/MISE/config.toml",
            "Mise/config.toml",
        ] {
            let found = stray_config_problems(&[file(path)]);
            assert_eq!(
                found,
                [format!(
                    "{path:?} is mise configuration beside mise.toml, which mise would merge over it"
                )],
                "{path}"
            );
        }
        for path in [
            "mise.toml",
            "mise.lock",
            "mise.semver.toml",
            "mise.semver.lock",
            ".config/nextest.toml",
            ".config",
            "Cargo.toml",
            "src/mise.local.toml",
        ] {
            assert_eq!(
                stray_config_problems(&[file(path)]),
                Vec::<String>::new(),
                "{path}"
            );
        }
    }

    /// The listing names every root entry and every file under the
    /// directories mise reads configuration from, whatever the case of the
    /// directory's name, and nothing deeper elsewhere.
    #[test]
    fn the_listing_reaches_every_configuration_directory() {
        let root = tempfile::tempdir().expect("a temporary directory");
        for file in [
            "mise.toml",
            ".config/nextest.toml",
            ".config/mise/conf.d/extra.toml",
            ".mise/config.toml",
            "Mise/config.toml",
            "src/mise.local.toml",
        ] {
            let path = root.path().join(file);
            std::fs::create_dir_all(path.parent().expect("a parent")).expect("a directory");
            std::fs::write(&path, "").expect("a file");
        }
        let listed = config_paths(root.path()).expect("the listing");
        for wanted in [
            "mise.toml",
            ".config",
            ".config/nextest.toml",
            ".config/mise",
            ".config/mise/conf.d/extra.toml",
            ".mise",
            ".mise/config.toml",
            "Mise",
            "Mise/config.toml",
            "src",
        ] {
            assert!(
                listed.iter().any(|entry| entry.path == wanted),
                "{wanted} in {listed:?}"
            );
        }
        assert!(
            !listed
                .iter()
                .any(|entry| entry.path == "src/mise.local.toml"),
            "{listed:?}"
        );
        assert!(listed.iter().all(|entry| !entry.link), "{listed:?}");
        let refused: Vec<&str> = listed
            .iter()
            .filter(|entry| !stray_config_problems(&[(*entry).clone()]).is_empty())
            .map(|entry| entry.path.as_str())
            .collect();
        assert_eq!(
            refused,
            [
                ".config/mise",
                ".config/mise/conf.d",
                ".config/mise/conf.d/extra.toml",
                ".mise",
                ".mise/config.toml",
                "Mise",
                "Mise/config.toml",
            ],
            "{listed:?}"
        );
    }

    /// Link `link` to the directory `target`.
    fn link_dir(target: &Path, link: &Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).expect("a link");
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(target, link).expect("a link");
    }

    /// A link is listed as a link, never walked, and refused whatever it
    /// names.
    ///
    /// mise follows a linked `.config` to the configuration behind it, which
    /// no name the rule checks would reach.
    #[test]
    fn a_link_is_refused_and_never_walked() {
        let root = tempfile::tempdir().expect("a temporary directory");
        for name in ["fixtures/mise.toml", "fixtures/mise/conf.d/extra.toml"] {
            let path = root.path().join(name);
            std::fs::create_dir_all(path.parent().expect("a parent")).expect("a directory");
            std::fs::write(&path, "").expect("a file");
        }
        link_dir(&root.path().join("fixtures"), &root.path().join(".config"));
        let listed = config_paths(root.path()).expect("the listing");
        assert!(
            listed.contains(&TreeEntry {
                path: ".config".to_string(),
                link: true
            }),
            "{listed:?}"
        );
        assert!(
            !listed
                .iter()
                .any(|entry| entry.path.starts_with(".config/")),
            "{listed:?}"
        );
        assert_eq!(
            stray_config_problems(&listed),
            ["\".config\" is a link, which mise would follow to configuration no rule here reads"]
        );
        let linked_pins = TreeEntry {
            path: "mise.toml".to_string(),
            link: true,
        };
        assert_eq!(
            stray_config_problems(&[linked_pins]),
            [
                "\"mise.toml\" is a link, which mise would follow to configuration no rule here reads"
            ]
        );
    }

    /// The pin file holds the tables the rules read, and its `[tool_config]`
    /// and `[settings]` are compared whole.
    ///
    /// mise runs a hook, exports an `[env]` table and honors a task from this
    /// file, and a setting turned off lifts a refusal mise makes. Each case
    /// passed every other rule here.
    #[test]
    fn the_pin_file_holds_the_tables_the_rules_read() {
        let lock = || SOUND_LOCK.to_string();
        let with = |extra: &str| format!("{}{extra}", sound_pins());
        let tool_config = format!("[tool_config]\n{TOOL_CONFIG}");
        let cases: &[(&str, String, String, &str)] = &[
            (
                "an env table",
                with("\n[env]\n_.path = [\"./bin\"]\n"),
                lock(),
                "mise.toml carries \"env\", and it holds [tools], [tool_config] and [settings] alone",
            ),
            (
                "a hooks table",
                with("\n[hooks]\npostinstall = \"echo\"\n"),
                lock(),
                "mise.toml carries \"hooks\", and it holds [tools], [tool_config] and [settings] alone",
            ),
            (
                "a task",
                with("\n[tasks.build]\nrun = \"echo\"\n"),
                lock(),
                "mise.toml carries \"tasks\", and it holds [tools], [tool_config] and [settings] alone",
            ),
            (
                "a key above every table",
                format!("min_version = \"2026.1.0\"\n{}", sound_pins()),
                lock(),
                "mise.toml carries \"min_version\", and it holds [tools], [tool_config] and [settings] alone",
            ),
            (
                "the tool_config table deleted",
                sound_pins().replace(&format!("{tool_config}\n\n"), ""),
                lock(),
                "mise.toml holds no [tool_config] table",
            ),
            (
                "the tool_config lock off",
                sound_pins().replace(&tool_config, "[tool_config]\nlocked = false"),
                lock(),
                "mise.toml does not set locked = true under [tool_config]",
            ),
            (
                "a key beside the tool_config lock",
                sound_pins().replace(
                    &tool_config,
                    "[tool_config]\nlocked = true\ndisable_backends = []",
                ),
                lock(),
                "mise.toml sets \"disable_backends\" under [tool_config], which no rule here reads",
            ),
            (
                "the settings lock off",
                sound_pins().replace("[settings]\nlocked = true", "[settings]\nlocked = false"),
                lock(),
                "mise.toml does not set locked = true under [settings]",
            ),
            (
                "the attestation check deleted",
                sound_pins().replace("locked_verify_provenance = true\n", ""),
                lock(),
                "mise.toml does not set locked_verify_provenance = true under [settings]",
            ),
            (
                "a failed lookup written as a string",
                sound_pins().replace(
                    "provenance_api_failures_fatal = true",
                    "provenance_api_failures_fatal = \"true\"",
                ),
                lock(),
                "mise.toml does not set provenance_api_failures_fatal = true under [settings]",
            ),
            (
                "a setting beside the ones read",
                sound_pins().replace("[settings]\n", "[settings]\nexperimental = true\n"),
                lock(),
                "mise.toml sets \"experimental\" under [settings], which no rule here reads",
            ),
        ];
        refuses(cases);
    }

    /// Both attestation checks are set true, the aqua one alone in its table,
    /// so a changed default cannot turn either off under the repository.
    #[test]
    fn the_pin_file_holds_both_attestation_checks() {
        let lock = || SOUND_LOCK.to_string();
        let cases: &[(&str, String, String, &str)] = &[
            (
                "the github attestation check off",
                sound_pins().replacen(
                    "github_attestations = true",
                    "github_attestations = false",
                    1,
                ),
                lock(),
                "mise.toml does not set github_attestations = true under [settings]",
            ),
            (
                "the aqua table deleted",
                sound_pins().replace("\n[settings.aqua]\ngithub_attestations = true\n", ""),
                lock(),
                "mise.toml holds no [settings.aqua] table",
            ),
            (
                "the aqua attestation check off",
                sound_pins().replace(
                    "[settings.aqua]\ngithub_attestations = true",
                    "[settings.aqua]\ngithub_attestations = false",
                ),
                lock(),
                "mise.toml does not set github_attestations = true under [settings.aqua]",
            ),
            (
                "a setting beside the aqua attestation check",
                sound_pins().replace(
                    "[settings.aqua]\ngithub_attestations = true",
                    "[settings.aqua]\ngithub_attestations = true\ncosign = false",
                ),
                lock(),
                "mise.toml sets \"cosign\" under [settings.aqua], which no rule here reads",
            ),
        ];
        refuses(cases);
    }

    /// A tool entry carries its version and its tag prefix and nothing else,
    /// in either pin file and in the options its lockfile entry records, and a
    /// lockfile carries only the keys mise writes.
    ///
    /// mise runs a postinstall command and exports an install environment from
    /// a tool entry. Each case passed every other rule here.
    #[test]
    fn a_tool_entry_carries_its_version_and_tag_prefix_alone() {
        let nextest = "\"github:nextest-rs/nextest\" = { version = \"0.9.145\", version_prefix = \"cargo-nextest-\" }";
        let taplo_api =
            "url_api = \"https://api.github.com/repos/tamasfe/taplo/releases/assets/257322600\"\n";
        let cases: &[(&str, String, String, &str)] = &[
            (
                "a postinstall command",
                sound_pins().replace(
                    "taplo = \"0.10.0\"",
                    "taplo = { version = \"0.10.0\", postinstall = \"echo\" }",
                ),
                SOUND_LOCK.to_string(),
                "mise.toml gives \"taplo\" the option \"postinstall\", and a tool here carries its version and tag prefix alone",
            ),
            (
                "an install environment",
                sound_pins().replace(
                    nextest,
                    &nextest.replace(" }", ", install_env = { A = \"b\" } }"),
                ),
                SOUND_LOCK.to_string(),
                "mise.toml gives \"github:nextest-rs/nextest\" the option \"install_env\", and a tool here carries its version and tag prefix alone",
            ),
            (
                "a tag prefix naming other tags",
                sound_pins().replace(
                    "version_prefix = \"cargo-nextest-\"",
                    "version_prefix = \"v\"",
                ),
                SOUND_LOCK.to_string(),
                "mise.toml gives \"github:nextest-rs/nextest\" the version_prefix \"v\", and its release tags start \"cargo-nextest-\"",
            ),
            (
                "an option in the lockfile alone",
                sound_pins(),
                SOUND_LOCK.replace(
                    "version_prefix = \"cargo-nextest-\"\n",
                    "version_prefix = \"cargo-nextest-\"\npostinstall = \"echo\"\n",
                ),
                "mise.lock gives \"github:nextest-rs/nextest\" the option \"postinstall\", and a tool here carries its version and tag prefix alone",
            ),
            (
                "a key on an entry mise does not write",
                sound_pins(),
                SOUND_LOCK.replace(
                    "specifiers = [\"0.9.145\"]\n",
                    "specifiers = [\"0.9.145\"]\ninstall_env = { A = \"b\" }\n",
                ),
                "mise.lock gives \"github:nextest-rs/nextest\" the key \"install_env\", which mise does not write",
            ),
            (
                "a key in a platform block mise does not write",
                sound_pins(),
                SOUND_LOCK.replace(taplo_api, &format!("{taplo_api}bin = \"other\"\n")),
                "mise.lock gives \"taplo\" on \"linux-x64\" the key \"bin\", which mise does not write",
            ),
            (
                "a key above every table",
                sound_pins(),
                format!("env = {{ A = \"b\" }}\n{SOUND_LOCK}"),
                "mise.lock carries \"env\", which mise does not write",
            ),
            (
                "another lockfile format",
                sound_pins(),
                SOUND_LOCK.replace("lockfile_version = 1\n", "lockfile_version = 2\n"),
                "mise.lock gives lockfile_version 2, and the rules here read version 1 alone",
            ),
            (
                "a format written as a string",
                sound_pins(),
                SOUND_LOCK.replace("lockfile_version = 1\n", "lockfile_version = \"1\"\n"),
                "mise.lock gives lockfile_version \"1\", and the rules here read version 1 alone",
            ),
            (
                "no lockfile format",
                sound_pins(),
                SOUND_LOCK.replace("lockfile_version = 1\n", ""),
                "mise.lock carries no lockfile_version, and the rules here read version 1 alone",
            ),
        ];
        refuses(cases);
        semver_refuses(&[(
            "a postinstall command in the semver file",
            sound_pins(),
            SOUND_SEMVER_PINS.replace(
                "= \"0.50.0\"",
                "= { version = \"0.50.0\", postinstall = \"echo\" }",
            ),
            SOUND_SEMVER_LOCK.to_string(),
            "mise.semver.toml gives \"github:obi1kenobi/cargo-semver-checks\" the option \"postinstall\", and a tool here carries its version and tag prefix alone",
        )]);
    }

    /// A control character in a value a finding prints reaches the finding as
    /// its escape, so a committed file writes no raw escape to a terminal and
    /// starts no line of its own in a CI log.
    #[test]
    fn a_finding_carries_no_raw_control_character() {
        let lock = format!(
            "{SOUND_LOCK}[[tools.\"probe\\n::error title=spoofed::pins passed\\u001b[8m\\u202edessap\"]]\nversion = \"1.0.0\"\n"
        );
        let found = problems(&sound_pins(), &lock);
        assert!(
            found.iter().any(|problem| problem
                .contains("probe\\n::error title=spoofed::pins passed\\u{1b}[8m\\u{202e}dessap")),
            "{found:?}"
        );
        assert!(
            found.iter().all(|problem| !problem.chars().any(hidden)),
            "{found:?}"
        );
    }
}
