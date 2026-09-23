//! How xtask starts a program: from an absolute path, and, for mise, in an
//! environment built from an allow-list.
//!
//! A bare program name resolves from the absolute entries of `PATH` alone, and
//! the resolved path is what runs. On Windows, `std::process::Command` searches
//! the running executable's own directory ahead of `PATH`, and `cargo run` puts
//! that directory and its `deps` directory at the front of `PATH` besides. Both
//! sit in cargo's build output, where a dependency's build script can write, so
//! an entry inside the running executable's directory or inside the checkout is
//! never searched, and every child started through [`command`] gets the same
//! narrowed `PATH`.
//!
//! mise reads its configuration, its data directory and its trust list from the
//! environment, so every mise child starts from a cleared one. It holds the pins
//! below and what mise needs to run, and nothing else: no `MISE_*` name reaches
//! mise unless this module sets it. On Windows the directories come from the
//! known folders, so an inherited variable cannot point mise at installs
//! somebody else chose. On Unix mise finds its installs through the inherited
//! home and XDG directories, which whoever sets them already controls.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// The variables every mise child runs with: `mise.toml` alone, with no
/// `.tool-versions`, no environment file and no per-platform file.
pub const MISE_PINS: &[(&str, &str)] = &[
    ("MISE_OVERRIDE_CONFIG_FILENAMES", crate::pins::PINS),
    ("MISE_OVERRIDE_TOOL_VERSIONS_FILENAMES", "none"),
    ("MISE_ENV", ""),
    ("MISE_AUTO_ENV", "false"),
];

/// The variable naming the one directory whose configuration mise trusts.
const TRUSTED: &str = "MISE_TRUSTED_CONFIG_PATHS";

/// The proxy settings a mise child keeps, in both spellings its HTTP client
/// reads. A certificate override is not among them.
const PROXIES: &[&str] = &[
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
    "NO_PROXY",
    "no_proxy",
];

/// The directories a mise child keeps on Unix, where mise finds its data,
/// cache and state through them. `XDG_CONFIG_HOME` is not among them, because
/// mise reads a global configuration file from it.
const UNIX_DIRS: &[&str] = &[
    "HOME",
    "TMPDIR",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
];

/// The inherited variables a mise child keeps on this platform.
fn inherited_names() -> Vec<&'static str> {
    let mut names = PROXIES.to_vec();
    if cfg!(unix) {
        names.extend_from_slice(UNIX_DIRS);
    }
    names
}

/// The Windows directories a mise child needs, read from the known folders.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "only Windows reads the known folders")
)]
pub struct Folders {
    /// The Windows directory, `SYSTEMROOT` in the child.
    pub windows: PathBuf,
    /// The user's local application data, `LOCALAPPDATA` in the child, where
    /// mise keeps its installs.
    pub local_app_data: PathBuf,
}

/// The absolute path the searched entries of `PATH` hold for the bare program
/// `name`.
///
/// # Errors
///
/// Returns the sentence a result row carries when `name` is not a bare name,
/// when the directories kept out of the search cannot be read, or when no
/// searched entry holds the program.
pub fn resolve(name: &str) -> Result<PathBuf, String> {
    resolve_in(name, &search_path()?)
}

/// A command running `program` with `PATH` narrowed to the entries [`resolve`]
/// searches, so a child that looks a program up by name, a nested xtask among
/// them, skips the same directories.
///
/// # Errors
///
/// Returns the sentence a result row carries when the directories kept out of
/// the search cannot be read or the narrowed `PATH` cannot be written.
pub fn command(program: &Path) -> Result<Command, String> {
    let path = std::env::join_paths(search_path()?)
        .map_err(|err| format!("PATH cannot be narrowed for {}: {err}", program.display()))?;
    let mut command = Command::new(program);
    command.env("PATH", path);
    Ok(command)
}

/// The path mise installs for `tool` in the checkout at `root`, or the
/// sentence saying why there is none.
///
/// `mise which` prints the path on the first line of standard output and any
/// warning on standard error, so reading standard output alone keeps the path
/// independent of how the two interleave. The environment is the allow-list
/// [`mise_environment`] builds, so no committed file and no `MISE_*` variable
/// chooses the binary.
///
/// # Errors
///
/// Returns the sentence a result row carries when mise cannot be found or run,
/// or when it resolves no `tool`, with the first line mise wrote to standard
/// error.
pub fn mise_which(root: &Path, tool: &str) -> Result<PathBuf, String> {
    let mise = resolve("mise")?;
    let output = mise_command(&mise, root)?
        .args(["which", tool])
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("running {}: {err}", mise.display()))?;
    which_answer(
        tool,
        output.status.success(),
        &output.stdout,
        &output.stderr,
    )
}

/// The path a `mise which` run for `tool` printed, or the sentence saying why
/// it printed none.
fn which_answer(
    tool: &str,
    success: bool,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<PathBuf, String> {
    let printed = String::from_utf8_lossy(stdout);
    let line = printed.lines().next().unwrap_or_default().trim();
    if success && !line.is_empty() {
        return Ok(PathBuf::from(line));
    }
    let said = String::from_utf8_lossy(stderr);
    let reason = said
        .lines()
        .map(|line| line.chars().filter(|c| !c.is_control()).collect::<String>())
        .map(|line| line.trim().to_string())
        .find(|line| !line.is_empty());
    Err(match reason {
        Some(reason) => format!("mise resolves no {tool} ({reason})"),
        None => format!("mise resolves no {tool}"),
    })
}

/// [`resolve`] against the searched `PATH` entries `entries`.
fn resolve_in(name: &str, entries: &[PathBuf]) -> Result<PathBuf, String> {
    let bare = !name.is_empty() && !name.contains(['/', '\\', ':']);
    if !bare {
        return Err(format!("{name:?} is not a bare program name"));
    }
    let missing = || format!("{name} is not on PATH");
    if entries.is_empty() {
        return Err(missing());
    }
    let joined = std::env::join_paths(entries).map_err(|_| missing())?;
    which::which_in_global(name, Some(joined))
        .map_err(|_| missing())?
        .find(|found| found.is_absolute() && starts_directly(found))
        .ok_or_else(missing)
}

/// Whether the operating system starts the file at `path` itself. On Windows
/// that is an `.exe`, the one kind `std::process::Command` looks for, and never
/// a script `cmd.exe` would interpret or an extensionless file.
fn starts_directly(path: &Path) -> bool {
    !cfg!(windows)
        || path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
}

/// The entries of this process's `PATH` that [`resolve`] searches, read once:
/// neither `PATH` nor the running executable changes while xtask runs, and each
/// read canonicalizes every entry.
fn search_path() -> Result<Vec<PathBuf>, String> {
    static SEARCH: OnceLock<Result<Vec<PathBuf>, String>> = OnceLock::new();
    SEARCH
        .get_or_init(|| {
            let excluded = excluded_roots(
                std::env::current_exe().ok().as_deref(),
                Path::new(env!("CARGO_MANIFEST_DIR")),
            )?;
            Ok(search_entries(
                std::env::var_os("PATH").as_deref(),
                &excluded,
            ))
        })
        .clone()
}

/// The directories no program resolves from, canonical: the one holding the
/// running executable at `exe`, which is cargo's build output, and the
/// checkout, the parent of the crate at `manifest_dir`, whose files a pull
/// request chooses.
fn excluded_roots(exe: Option<&Path>, manifest_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let output = exe
        .and_then(Path::parent)
        .ok_or("the running executable's directory is unknown")?;
    let checkout = manifest_dir
        .parent()
        .ok_or("the checkout holding xtask is unknown")?;
    [output, checkout]
        .into_iter()
        .map(|dir| {
            dir.canonicalize()
                .map_err(|err| format!("reading {}: {err}", dir.display()))
        })
        .collect()
}

/// The entries of the `PATH` value `path` a program resolves from, in order:
/// absolute, naming a directory that exists, and outside every canonical
/// directory in `excluded`. An empty or relative entry would resolve against
/// the working directory, which a checkout controls. The comparison is on the
/// canonical path, so neither spelling nor a link reaches an excluded
/// directory.
fn search_entries(path: Option<&OsStr>, excluded: &[PathBuf]) -> Vec<PathBuf> {
    path.iter()
        .flat_map(std::env::split_paths)
        .filter(|entry| entry.is_absolute())
        .filter(|entry| {
            entry
                .canonicalize()
                .is_ok_and(|real| !excluded.iter().any(|root| real.starts_with(root)))
        })
        .collect()
}

/// A command running the mise binary at `mise` in the checkout at `root`, with
/// its environment cleared and rebuilt by [`mise_environment`].
#[cfg_attr(
    not(windows),
    allow(
        clippy::unnecessary_wraps,
        reason = "only Windows reads the known folders, the one step that fails"
    )
)]
fn mise_command(mise: &Path, root: &Path) -> Result<Command, String> {
    #[cfg(windows)]
    let folders = Some(known_folder::folders()?);
    #[cfg(not(windows))]
    let folders = None;
    let environment = mise_environment(root, folders.as_ref(), &inherited_names(), |name| {
        std::env::var_os(name)
    });
    let mut command = Command::new(mise);
    command.env_clear().envs(environment).current_dir(root);
    Ok(command)
}

/// Every variable a mise child runs with: the pins, the checkout as the one
/// trusted configuration path, the Windows directories from `folders`, and
/// each of `names` that `inherited` answers.
fn mise_environment(
    root: &Path,
    folders: Option<&Folders>,
    names: &[&str],
    inherited: impl Fn(&str) -> Option<OsString>,
) -> Vec<(OsString, OsString)> {
    let mut environment: Vec<(OsString, OsString)> = MISE_PINS
        .iter()
        .map(|(name, value)| ((*name).into(), (*value).into()))
        .collect();
    environment.push((TRUSTED.into(), root.as_os_str().to_owned()));
    if let Some(folders) = folders {
        let temp = folders.local_app_data.join("Temp").into_os_string();
        environment.push((
            "SYSTEMROOT".into(),
            folders.windows.clone().into_os_string(),
        ));
        environment.push((
            "LOCALAPPDATA".into(),
            folders.local_app_data.clone().into_os_string(),
        ));
        environment.push(("TEMP".into(), temp.clone()));
        environment.push(("TMP".into(), temp));
    }
    for name in names {
        if let Some(value) = inherited(name) {
            environment.push(((*name).into(), value));
        }
    }
    environment
}

/// The one call that reads a Windows known folder.
#[cfg(windows)]
#[allow(
    unsafe_code,
    reason = "SHGetKnownFolderPath, the one Win32 call that reads a known folder"
)]
mod known_folder {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use std::path::PathBuf;

    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{
        FOLDERID_LocalAppData, FOLDERID_Windows, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
    };
    use windows_sys::core::{GUID, PWSTR};

    /// The Windows directory and the local application data directory.
    pub(super) fn folders() -> Result<super::Folders, String> {
        Ok(super::Folders {
            windows: known_folder(&FOLDERID_Windows, "the Windows directory")?,
            local_app_data: known_folder(&FOLDERID_LocalAppData, "the local application data")?,
        })
    }

    /// The path Windows records for the known folder `id`.
    fn known_folder(id: &GUID, what: &str) -> Result<PathBuf, String> {
        let mut raw: PWSTR = std::ptr::null_mut();
        let flags = KF_FLAG_DEFAULT.cast_unsigned();
        // SAFETY: `id` points at a GUID that outlives the call, a null token
        // asks for the current user, and `raw` is a valid place for the call to
        // store the string it allocates.
        let status = unsafe { SHGetKnownFolderPath(id, flags, std::ptr::null_mut(), &raw mut raw) };
        let path = (status == 0 && !raw.is_null()).then(|| {
            let mut len = 0;
            // SAFETY: on success `raw` is a NUL-terminated wide string, so every
            // read up to and including the terminator stays inside it.
            while unsafe { *raw.wrapping_add(len) } != 0 {
                len += 1;
            }
            // SAFETY: the `len` units before the terminator were just read and
            // are initialized, and nothing writes to them while the slice lives.
            let wide = unsafe { std::slice::from_raw_parts(raw, len) };
            PathBuf::from(OsString::from_wide(wide))
        });
        // SAFETY: `raw` is null or the buffer the call allocated with
        // CoTaskMemAlloc, freed once here, and CoTaskMemFree accepts null.
        unsafe { CoTaskMemFree(raw.cast()) };
        path.ok_or_else(|| format!("Windows reports no path for {what} (HRESULT {status:#010x})"))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Folders, MISE_PINS, PROXIES, UNIX_DIRS, excluded_roots, inherited_names, mise_environment,
        resolve_in, search_entries, which_answer,
    };
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    /// A file the resolver accepts as the program `name` in `dir`: an `.exe`
    /// on Windows, where `PATHEXT` decides, and an executable bit on Unix.
    fn plant(dir: &Path, name: &str) -> PathBuf {
        let file = if cfg!(windows) {
            dir.join(format!("{name}.exe"))
        } else {
            dir.join(name)
        };
        std::fs::write(&file, "").expect("a file");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755))
                .expect("an executable bit");
        }
        file
    }

    /// A bare name resolves to the absolute path an absolute entry holds.
    #[test]
    fn a_bare_name_resolves_from_an_absolute_entry() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let planted = plant(dir.path(), "xtask-probe-tool");
        let found =
            resolve_in("xtask-probe-tool", &[dir.path().to_path_buf()]).expect("the planted tool");
        assert!(found.is_absolute(), "{found:?}");
        assert_eq!(
            found.canonicalize().expect("the found path"),
            planted.canonicalize().expect("the planted path")
        );
    }

    /// An empty, relative or missing `PATH` entry is dropped, so the working
    /// directory a checkout controls is never searched.
    #[test]
    fn only_absolute_entries_are_searched() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let absolute = dir.path().to_path_buf();
        let path = std::env::join_paths([
            PathBuf::from("."),
            PathBuf::new(),
            PathBuf::from("relative"),
            absolute.clone(),
            PathBuf::from("target/debug"),
            absolute.join("missing"),
        ])
        .expect("a PATH value");
        assert_eq!(search_entries(Some(path.as_os_str()), &[]), [absolute]);
        assert_eq!(search_entries(None, &[]), Vec::<PathBuf>::new());
    }

    /// The directory holding the running executable, with the `deps` directory
    /// inside it that `cargo run` also puts on `PATH`, and the checkout are
    /// never searched, however an entry spells them.
    #[test]
    fn the_build_output_and_the_checkout_are_never_searched() {
        let outside = tempfile::tempdir().expect("a temporary directory");
        let output = outside.path().join("debug");
        let deps = output.join("deps");
        let tools = outside.path().join("tools");
        std::fs::create_dir_all(&deps).expect("the build output");
        std::fs::create_dir_all(&tools).expect("a tool directory");
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let excluded = excluded_roots(Some(&output.join("xtask.exe")), manifest_dir)
            .expect("the excluded directories");
        let respelled = PathBuf::from(deps.to_string_lossy().to_uppercase());
        let path = std::env::join_paths([
            output.clone(),
            deps.clone(),
            respelled,
            manifest_dir.to_path_buf(),
            tools.clone(),
        ])
        .expect("a PATH value");
        assert_eq!(search_entries(Some(path.as_os_str()), &excluded), [tools]);
    }

    /// A child runs with `PATH` set, and none of its entries sits inside the
    /// checkout, where `cargo test` puts its build output on Windows.
    #[test]
    fn a_child_gets_the_narrowed_path() {
        let command = super::command(Path::new("child")).expect("a command");
        let path = command
            .get_envs()
            .find(|(name, _)| *name == "PATH")
            .and_then(|(_, value)| value)
            .expect("PATH is set");
        let checkout = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the checkout")
            .canonicalize()
            .expect("the canonical checkout");
        for entry in std::env::split_paths(path) {
            let real = entry.canonicalize().expect("an entry naming a directory");
            assert!(!real.starts_with(&checkout), "{entry:?}");
        }
    }

    /// Windows starts an `.exe` found on `PATH` and nothing else: a script or
    /// an extensionless file earlier on `PATH` is passed over.
    #[cfg(windows)]
    #[test]
    fn only_an_exe_resolves_on_windows() {
        let first = tempfile::tempdir().expect("a temporary directory");
        let second = tempfile::tempdir().expect("a temporary directory");
        for script in [
            "xtask-probe-tool",
            "xtask-probe-tool.com",
            "xtask-probe-tool.cmd",
        ] {
            std::fs::write(first.path().join(script), "").expect("a script");
        }
        let planted = plant(second.path(), "xtask-probe-tool");
        let entries = [first.path().to_path_buf(), second.path().to_path_buf()];
        let found = resolve_in("xtask-probe-tool", &entries).expect("the planted tool");
        assert_eq!(
            found.canonicalize().expect("the found path"),
            planted.canonicalize().expect("the planted path")
        );
        assert_eq!(
            resolve_in("xtask-probe-tool", &entries[..1]),
            Err("xtask-probe-tool is not on PATH".to_string())
        );
    }

    /// A name that names a path rather than a program is refused, and the
    /// refusal says so.
    #[test]
    fn a_name_with_a_separator_is_refused() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let entries = [dir.path().to_path_buf()];
        for name in ["", "sub/tool", "..\\tool", "C:tool", "./tool"] {
            assert_eq!(
                resolve_in(name, &entries),
                Err(format!("{name:?} is not a bare program name")),
                "{name:?}"
            );
        }
    }

    /// A program no searched entry holds is refused by name.
    #[test]
    fn a_missing_program_is_named() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        assert_eq!(
            resolve_in("xtask-probe-missing", &[dir.path().to_path_buf()]),
            Err("xtask-probe-missing is not on PATH".to_string())
        );
        assert_eq!(
            resolve_in("xtask-probe-missing", &[]),
            Err("xtask-probe-missing is not on PATH".to_string())
        );
    }

    /// `mise which` answers with its first line when it succeeds, and a refusal
    /// carries the first line mise wrote to standard error.
    #[test]
    fn a_which_answer_keeps_the_reason() {
        let cases: [(bool, &str, &str, Result<PathBuf, String>); 5] = [
            (
                true,
                "/mise/installs/taplo/taplo\n",
                "mise WARN one\n",
                Ok(PathBuf::from("/mise/installs/taplo/taplo")),
            ),
            (
                false,
                "",
                "\n  mise ERROR taplo is not installed\u{1b}\nsecond\n",
                Err("mise resolves no taplo (mise ERROR taplo is not installed)".to_string()),
            ),
            (false, "", "", Err("mise resolves no taplo".to_string())),
            (true, "\n", "", Err("mise resolves no taplo".to_string())),
            (
                false,
                "/mise/installs/taplo/taplo\n",
                "",
                Err("mise resolves no taplo".to_string()),
            ),
        ];
        for (success, stdout, stderr, wanted) in cases {
            assert_eq!(
                which_answer("taplo", success, stdout.as_bytes(), stderr.as_bytes()),
                wanted,
                "{success} {stdout:?} {stderr:?}"
            );
        }
    }

    /// The Windows directories the builder is given, as the known folders
    /// would answer.
    fn folders() -> Folders {
        Folders {
            windows: PathBuf::from("C:\\Windows"),
            local_app_data: PathBuf::from("C:\\Users\\probe\\AppData\\Local"),
        }
    }

    /// An inherited environment holding every allowed name and every name the
    /// builder has to keep out.
    fn inherited(name: &str) -> Option<OsString> {
        let value = match name {
            "HTTPS_PROXY" | "https_proxy" | "HTTP_PROXY" | "http_proxy" | "NO_PROXY"
            | "no_proxy" => format!("{name}-value"),
            "HOME" | "TMPDIR" | "XDG_DATA_HOME" | "XDG_CACHE_HOME" | "XDG_STATE_HOME" => {
                format!("/probe/{name}")
            }
            "LOCALAPPDATA"
            | "SYSTEMROOT"
            | "TEMP"
            | "TMP"
            | "MISE_DATA_DIR"
            | "MISE_GLOBAL_CONFIG_FILE"
            | "MISE_TRUSTED_CONFIG_PATHS"
            | "MISE_ENV"
            | "GITHUB_TOKEN"
            | "MISE_GITHUB_TOKEN"
            | "SSL_CERT_FILE"
            | "XDG_CONFIG_HOME"
            | "PATH" => "/attacker/chosen".to_string(),
            _ => return None,
        };
        Some(value.into())
    }

    /// The builder's output as a map, so a test reads each value by name.
    fn built(folders: Option<&Folders>, names: &[&str]) -> BTreeMap<String, String> {
        mise_environment(Path::new("/checkout"), folders, names, inherited)
            .into_iter()
            .map(|(name, value)| {
                (
                    name.into_string().expect("a UTF-8 name"),
                    value.into_string().expect("a UTF-8 value"),
                )
            })
            .collect()
    }

    /// On Windows the child holds the pins, the checkout as the one trusted
    /// path, the known folders and the proxies, and nothing else: every
    /// inherited directory, token and certificate override stays out.
    #[test]
    fn the_windows_environment_is_the_allow_list() {
        let folders = folders();
        let environment = built(Some(&folders), PROXIES);
        let mut wanted: BTreeMap<String, String> = MISE_PINS
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect();
        wanted.insert("MISE_TRUSTED_CONFIG_PATHS".into(), "/checkout".into());
        wanted.insert("SYSTEMROOT".into(), "C:\\Windows".into());
        wanted.insert(
            "LOCALAPPDATA".into(),
            "C:\\Users\\probe\\AppData\\Local".into(),
        );
        wanted.insert(
            "TEMP".into(),
            "C:\\Users\\probe\\AppData\\Local\\Temp".into(),
        );
        wanted.insert(
            "TMP".into(),
            "C:\\Users\\probe\\AppData\\Local\\Temp".into(),
        );
        for name in PROXIES {
            wanted.insert((*name).into(), format!("{name}-value"));
        }
        assert_eq!(environment, wanted);
    }

    /// On Unix the child keeps the directories mise finds its installs
    /// through, and still nothing mise reads configuration or a token from.
    #[test]
    fn the_unix_environment_is_the_allow_list() {
        let names: Vec<&str> = PROXIES.iter().chain(UNIX_DIRS).copied().collect();
        let environment = built(None, &names);
        let mut wanted: BTreeMap<String, String> = MISE_PINS
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect();
        wanted.insert("MISE_TRUSTED_CONFIG_PATHS".into(), "/checkout".into());
        for name in PROXIES {
            wanted.insert((*name).into(), format!("{name}-value"));
        }
        for name in UNIX_DIRS {
            wanted.insert((*name).into(), format!("/probe/{name}"));
        }
        assert_eq!(environment, wanted);
    }

    /// The inherited proxy names and Unix directories are exactly these.
    #[test]
    fn the_inherited_lists_are_these() {
        assert_eq!(
            PROXIES,
            [
                "HTTPS_PROXY",
                "https_proxy",
                "HTTP_PROXY",
                "http_proxy",
                "NO_PROXY",
                "no_proxy",
            ]
        );
        assert_eq!(
            UNIX_DIRS,
            [
                "HOME",
                "TMPDIR",
                "XDG_DATA_HOME",
                "XDG_CACHE_HOME",
                "XDG_STATE_HOME",
            ]
        );
    }

    /// `resolve` searches the narrowed entries: the running test binary sits
    /// in a directory `cargo test` puts on `PATH`, and it does not resolve.
    #[cfg(windows)]
    #[test]
    fn resolve_skips_the_build_output_on_path() {
        let exe = std::env::current_exe().expect("the test binary");
        let dir = exe
            .parent()
            .expect("its directory")
            .canonicalize()
            .expect("its canonical directory");
        let path = std::env::var_os("PATH").expect("a PATH");
        assert!(
            std::env::split_paths(&path)
                .any(|entry| entry.canonicalize().is_ok_and(|real| real == dir)),
            "the test binary's directory is on PATH"
        );
        let stem = exe
            .file_stem()
            .and_then(|stem| stem.to_str())
            .expect("a UTF-8 name");
        assert_eq!(super::resolve(stem), Err(format!("{stem} is not on PATH")));
    }

    /// A link into an excluded directory is dropped, because the comparison
    /// is on the canonical path.
    #[cfg(unix)]
    #[test]
    fn a_link_into_the_build_output_is_dropped() {
        let outside = tempfile::tempdir().expect("a temporary directory");
        let output = outside.path().join("debug");
        let link = outside.path().join("link");
        let tools = outside.path().join("tools");
        std::fs::create_dir_all(&output).expect("the build output");
        std::fs::create_dir_all(&tools).expect("a tool directory");
        std::os::unix::fs::symlink(&output, &link).expect("a link to the build output");
        let excluded = excluded_roots(
            Some(&output.join("xtask")),
            Path::new(env!("CARGO_MANIFEST_DIR")),
        )
        .expect("the excluded directories");
        let path = std::env::join_paths([link, tools.clone()]).expect("a PATH value");
        assert_eq!(search_entries(Some(path.as_os_str()), &excluded), [tools]);
    }

    /// The pins hold mise to mise.toml alone.
    #[test]
    fn the_pins_hold_mise_to_one_file() {
        assert_eq!(
            MISE_PINS,
            [
                ("MISE_OVERRIDE_CONFIG_FILENAMES", "mise.toml"),
                ("MISE_OVERRIDE_TOOL_VERSIONS_FILENAMES", "none"),
                ("MISE_ENV", ""),
                ("MISE_AUTO_ENV", "false"),
            ]
        );
    }

    /// The inherited names follow the platform: the Unix directories only
    /// where mise reads them.
    #[test]
    fn the_inherited_names_follow_the_platform() {
        let names = inherited_names();
        for name in PROXIES {
            assert!(names.contains(name), "{name}");
        }
        for name in UNIX_DIRS {
            assert_eq!(names.contains(name), cfg!(unix), "{name}");
        }
        assert!(!names.contains(&"XDG_CONFIG_HOME"));
    }

    /// Windows answers both known folders with absolute directories that
    /// exist.
    #[cfg(windows)]
    #[test]
    fn the_known_folders_are_real_directories() {
        let folders = super::known_folder::folders().expect("the known folders");
        for path in [&folders.windows, &folders.local_app_data] {
            assert!(path.is_absolute() && path.is_dir(), "{path:?}");
        }
    }
}
