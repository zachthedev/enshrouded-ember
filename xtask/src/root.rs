//! The directory the dev loop keeps its state under.
//!
//! Everything a contributor fetches or extracts lands under `<root>/.cache`,
//! which `.gitignore` excludes. Every byte in it is regenerable by a fetch or
//! an extract. The root is the workspace root by default, `EMBER_DEV_ROOT`
//! overrides that, and `--root` overrides both. A sibling checkout with a
//! server already fetched is used by pointing `--root` at that repository
//! rather than by copying anything.
//!
//! A build is named by its Steam buildid, which is the key `SteamCMD`, the
//! depot manifest, the store page and the build watcher all already speak.
//!
//! Every directory this module hands out that a command will write into goes
//! through the Steam refusal first: the build directory, the run directory and
//! the extraction directory alike. A name that is not one plain path segment
//! never reaches a join, because `Path::join` replaces its base when the
//! argument is absolute.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// The environment variable that overrides the workspace root.
pub const ROOT_VAR: &str = "EMBER_DEV_ROOT";

/// The directory every regenerable artifact lives under.
pub const CACHE_DIR: &str = ".cache";

/// The Steam application the dedicated server ships as.
pub const APP_ID: &str = "2278520";

/// The Steam application the game client ships as.
///
/// A separate application from the dedicated server, with a buildid of its own
/// that rises independently, so the two numbers are never comparable.
pub const CLIENT_APP_ID: &str = "1203620";

/// Names Windows reserves for devices, which no directory can carry.
const RESERVED_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// A resolved dev root.
pub struct DevRoot {
    path: PathBuf,
}

impl DevRoot {
    /// Decide the root for this run.
    ///
    /// # Errors
    ///
    /// Returns an error when the root resolves inside a Steam library, which
    /// would put fetched builds and run state inside a Steam install.
    pub fn resolve(explicit: Option<&Path>) -> Result<Self> {
        let path = match explicit {
            Some(path) => path.to_path_buf(),
            None => match std::env::var_os(ROOT_VAR) {
                Some(value) => PathBuf::from(value),
                None => crate::workspace_root(),
            },
        };
        let path = std::path::absolute(&path)
            .with_context(|| format!("resolving the dev root at {}", path.display()))?;
        crate::steam::ensure_outside_library(&path, "the dev root")?;
        Ok(Self { path })
    }

    /// The gitignored directory every fetched and derived artifact lives under.
    pub fn cache(&self) -> PathBuf {
        self.path.join(CACHE_DIR)
    }

    /// Where fetched server builds live, one directory per buildid.
    pub fn server_dir(&self) -> PathBuf {
        self.cache().join("server")
    }

    /// Where extractions live, one directory per buildid.
    pub fn schema_dir(&self) -> PathBuf {
        self.cache().join("schema")
    }

    /// Where localization extractions live, one directory per buildid.
    pub fn loca_dir(&self) -> PathBuf {
        self.cache().join("loca")
    }

    /// Where `SteamCMD` lives.
    pub fn steamcmd(&self) -> PathBuf {
        self.cache().join("steamcmd").join("steamcmd.exe")
    }

    /// The directory a named build would occupy, fetched or not.
    ///
    /// # Errors
    ///
    /// Returns an error when the buildid is not a single path segment, or when
    /// the directory resolves inside a Steam library.
    pub fn build_dir(&self, build: &str) -> Result<PathBuf> {
        check_build_id(build)?;
        let path = self.server_dir().join(build);
        crate::steam::ensure_outside_library(&path, "a server directory")?;
        Ok(path)
    }

    /// The directory holding one build's saves, config, log and pid file.
    ///
    /// Run state is kept beside the fetched build rather than inside it, so a
    /// build stays as `SteamCMD` wrote it and a validate run has nothing to
    /// repair.
    ///
    /// # Errors
    ///
    /// Returns an error when the buildid is not a single path segment, or when
    /// the directory resolves inside a Steam library.
    pub fn run_dir(&self, build: &str) -> Result<PathBuf> {
        check_build_id(build)?;
        let path = self.cache().join("run").join(build);
        crate::steam::ensure_outside_library(&path, "a run directory")?;
        Ok(path)
    }

    /// The directory holding one build's extraction.
    ///
    /// # Errors
    ///
    /// Returns an error when the buildid is not a single path segment, or when
    /// the directory resolves inside a Steam library.
    pub fn schema_build_dir(&self, build: &str) -> Result<PathBuf> {
        check_build_id(build)?;
        let path = self.schema_dir().join(build);
        crate::steam::ensure_outside_library(&path, "an extraction directory")?;
        Ok(path)
    }

    /// The directory holding one build's localization tables.
    ///
    /// # Errors
    ///
    /// Returns an error when the buildid is not a single path segment, or when
    /// the directory resolves inside a Steam library.
    pub fn loca_build_dir(&self, build: &str) -> Result<PathBuf> {
        check_build_id(build)?;
        let path = self.loca_dir().join(build);
        crate::steam::ensure_outside_library(&path, "a localization directory")?;
        Ok(path)
    }

    /// The buildid a command works on.
    ///
    /// A named buildid is used as given. With none named, the newest fetched
    /// build is used, because buildids rise with every Steam release.
    ///
    /// # Errors
    ///
    /// Returns an error when nothing is fetched, or when the buildid is not a
    /// single path segment.
    pub fn build_id(&self, build: Option<&str>) -> Result<String> {
        if let Some(named) = build {
            check_build_id(named)?;
            return Ok(named.to_string());
        }
        let fetched = self.fetched_builds();
        match fetched.last() {
            Some(newest) => Ok(newest.clone()),
            None => bail!(
                "no server build under {}. Run `cargo xtask server fetch` first.",
                self.server_dir().display()
            ),
        }
    }

    /// Every fetched buildid, oldest first.
    ///
    /// Only a name that is a whole number counts, sorted by value so build
    /// 10000000 follows build 9999999. A staging directory, a hand-placed
    /// directory or anything else beside the builds is never a candidate.
    pub fn fetched_builds(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(self.server_dir()) else {
            return Vec::new();
        };
        let mut numbered: Vec<(u64, String)> = entries
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                name.parse::<u64>().ok().map(|value| (value, name))
            })
            .collect();
        numbered.sort_unstable();
        numbered.into_iter().map(|(_, name)| name).collect()
    }
}

/// Refuse a buildid that is anything but one plain directory name.
///
/// The accepted alphabet is ASCII letters, digits, dot, hyphen and underscore,
/// with no leading dot, no trailing dot or space, and none of the names
/// Windows reserves for devices. Everything Steam has ever issued is a whole
/// number, so the alphabet is wider than any real buildid and narrower than
/// anything a filesystem could mistake for a path.
///
/// # Errors
///
/// Returns an error naming what was refused.
pub fn check_build_id(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("a buildid cannot be empty");
    }
    if let Some(bad) = name
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')))
    {
        bail!("a buildid is one directory name, so {name:?} cannot carry {bad:?}");
    }
    if name.starts_with('.') {
        bail!("a buildid cannot start with a dot, so {name:?} cannot be used");
    }
    if name.ends_with('.') || name.ends_with(' ') {
        bail!("a buildid cannot end in a dot or a space, so {name:?} cannot be used");
    }
    let stem = name.split('.').next().unwrap_or(name);
    if RESERVED_NAMES
        .iter()
        .any(|reserved| stem.eq_ignore_ascii_case(reserved))
    {
        bail!("{name:?} is a name Windows reserves for a device");
    }
    Ok(())
}

/// The buildid an app manifest under `install_dir` records for one application.
///
/// The value is checked the way a typed buildid is, because it becomes a
/// directory name the same way.
///
/// # Errors
///
/// Returns an error when the manifest is absent, carries no buildid, or
/// carries one that is not a plain directory name.
pub fn build_id_from_manifest(install_dir: &Path, app: &str) -> Result<String> {
    let manifest = install_dir
        .join("steamapps")
        .join(format!("appmanifest_{app}.acf"));
    let text = std::fs::read_to_string(&manifest)
        .with_context(|| format!("reading {}", manifest.display()))?;
    for line in text.lines() {
        let mut quoted = line.split('"').skip(1);
        let (Some(key), Some(value)) = (quoted.next(), quoted.nth(1)) else {
            continue;
        };
        if key == "buildid" && !value.is_empty() {
            check_build_id(value).with_context(|| {
                format!("{} names a buildid that is not usable", manifest.display())
            })?;
            return Ok(value.to_string());
        }
    }
    bail!("{} names no buildid", manifest.display())
}

#[cfg(test)]
mod tests {
    use super::{DevRoot, build_id_from_manifest, check_build_id};
    use crate::testutil::TestDir;

    #[test]
    fn a_buildid_is_one_plain_directory_name() {
        let cases: &[(&str, bool)] = &[
            ("23178631", true),
            ("23178631-local", true),
            ("build_2", true),
            ("v1.0", true),
            ("", false),
            (".", false),
            ("..", false),
            (".fetching", false),
            ("../escape", false),
            ("..\\escape", false),
            ("nested/build", false),
            ("D:current", false),
            ("C:\\absolute", false),
            ("23178631.", false),
            ("23178631 ", false),
            ("with space", false),
            ("NUL", false),
            ("nul", false),
            ("con.txt", false),
            ("COM1", false),
            ("lpt9", false),
            ("Ünïcode", false),
        ];
        for (name, want) in cases {
            assert_eq!(check_build_id(name).is_ok(), *want, "name {name:?}");
        }
    }

    #[test]
    fn the_buildid_comes_from_the_app_manifest() {
        let dir = TestDir::new("manifest");
        dir.write(
            "steamapps/appmanifest_2278520.acf",
            b"\"AppState\"\n{\n\t\"appid\"\t\t\"2278520\"\n\t\"name\"\t\t\"Enshrouded Dedicated Server\"\n\t\"buildid\"\t\t\"23178631\"\n}\n",
        );
        assert_eq!(
            build_id_from_manifest(dir.path(), super::APP_ID).unwrap(),
            "23178631"
        );
    }

    /// A manifest is a file an operator can edit, so its buildid is held to
    /// the same rule as a typed one before it becomes a directory name.
    #[test]
    fn a_manifest_buildid_that_escapes_is_refused() {
        let cases: &[&str] = &[
            "..\\..\\x",
            "../../x",
            "C:\\SteamLibrary",
            ".fetching",
            "NUL",
        ];
        for value in cases {
            let dir = TestDir::new("manifest-escape");
            dir.write(
                "steamapps/appmanifest_2278520.acf",
                format!("\"AppState\"\n{{\n\t\"buildid\"\t\t\"{value}\"\n}}\n").as_bytes(),
            );
            let err = build_id_from_manifest(dir.path(), super::APP_ID).expect_err(value);
            assert!(
                format!("{err:#}").contains("not usable"),
                "{value}: {err:#}"
            );
        }
    }

    #[test]
    fn a_manifest_with_no_buildid_is_an_error() {
        let dir = TestDir::new("manifest-empty");
        dir.write(
            "steamapps/appmanifest_2278520.acf",
            b"\"AppState\"\n{\n\t\"appid\"\t\t\"2278520\"\n}\n",
        );
        let err =
            build_id_from_manifest(dir.path(), super::APP_ID).expect_err("no buildid to read");
        assert!(format!("{err}").contains("names no buildid"), "{err}");
    }

    #[test]
    fn a_missing_manifest_names_the_path_it_looked_for() {
        let dir = TestDir::new("manifest-missing");
        let err = build_id_from_manifest(dir.path(), super::APP_ID).expect_err("nothing to read");
        assert!(
            format!("{err:#}").contains("appmanifest_2278520.acf"),
            "{err:#}"
        );
    }

    /// A staging directory left by an interrupted fetch, or anything else
    /// beside the builds, is never the default build.
    #[test]
    fn only_a_numbered_directory_is_a_fetched_build() {
        let dir = TestDir::new("fetched");
        let root = DevRoot {
            path: dir.path().to_path_buf(),
        };
        let server = root.server_dir();
        for name in ["23178631", ".fetching", "hand-placed", "9999999"] {
            std::fs::create_dir_all(server.join(name)).unwrap();
        }
        assert_eq!(root.fetched_builds(), vec!["9999999", "23178631"]);
        assert_eq!(root.build_id(None).unwrap(), "23178631");

        std::fs::remove_dir_all(server.join("23178631")).unwrap();
        std::fs::remove_dir_all(server.join("9999999")).unwrap();
        assert!(root.fetched_builds().is_empty());
        let err = root
            .build_id(None)
            .expect_err("nothing numbered is fetched");
        assert!(format!("{err}").contains("server fetch"), "{err}");
    }

    /// Every directory a command writes into goes through the Steam refusal,
    /// the extraction and run directories included.
    #[test]
    fn every_writable_directory_is_refused_inside_a_library() {
        let dir = TestDir::new("root-guard");
        dir.write("fake/SteamLibrary/steamapps/common/keep", b"");
        let library = dir.path().join("fake").join("SteamLibrary");
        let root = DevRoot { path: library };
        let cases: [(&str, anyhow::Result<std::path::PathBuf>); 4] = [
            ("build", root.build_dir("23178631")),
            ("run", root.run_dir("23178631")),
            ("schema", root.schema_build_dir("23178631")),
            ("loca", root.loca_build_dir("23178631")),
        ];
        for (label, outcome) in cases {
            let err = outcome.expect_err(label);
            assert!(format!("{err}").contains("Steam library"), "{label}: {err}");
        }
    }

    /// A buildid that would leave the cache is refused before any join, on
    /// every directory kind.
    #[test]
    fn an_escaping_buildid_never_reaches_a_join() {
        let dir = TestDir::new("root-escape");
        let root = DevRoot {
            path: dir.path().to_path_buf(),
        };
        for name in ["..\\..\\out", "C:\\SteamLibrary", "/abs", ".fetching"] {
            assert!(root.build_dir(name).is_err(), "build_dir {name}");
            assert!(root.run_dir(name).is_err(), "run_dir {name}");
            assert!(
                root.schema_build_dir(name).is_err(),
                "schema_build_dir {name}"
            );
            assert!(root.loca_build_dir(name).is_err(), "loca_build_dir {name}");
            assert!(root.build_id(Some(name)).is_err(), "build_id {name}");
        }
    }
}
