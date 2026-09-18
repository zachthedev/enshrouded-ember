//! Refusal of Steam library paths.
//!
//! A Steam install of Enshrouded is read-only reference. Nothing in this
//! repository writes into one, launches a binary out of one, or injects into a
//! process started from one. Every command that takes a server directory sends
//! it through [`ensure_outside_library`] first.
//!
//! Matching is structural, not textual. The path is made absolute, the longest
//! existing prefix is canonicalized so a junction or a symlink resolves to what
//! it points at, and the result is judged by the shape of a Steam library.
//!
//! What makes a library is `steamapps` with `common` or `libraryfolders.vdf`
//! under it. A `SteamCMD` install directory also carries a `steamapps` directory,
//! holding nothing but app manifests, so the narrower rule is what lets a
//! fetched build sit in the cache without refusing itself.

use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};

use anyhow::{Result, bail};

/// The directory name every Steam library keeps its content under.
const STEAMAPPS: &str = "steamapps";

/// The two children that make a `steamapps` directory a library rather than a
/// `SteamCMD` install target.
const LIBRARY_MARKERS: [&str; 2] = ["common", "libraryfolders.vdf"];

/// Refuse `path` when it resolves inside a Steam library.
///
/// # Errors
///
/// Returns an error naming the path, the Steam library that matched, and why
/// the refusal exists.
pub fn ensure_outside_library(path: &Path, purpose: &str) -> Result<()> {
    // The `steamapps/common` pair settles it with no filesystem behind the
    // path, which is what keeps a refusal off the network when the path is a
    // UNC share that is not reachable.
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let resolved = match content_tree(&absolute) {
        Some(_) => absolute,
        None => resolve(path),
    };
    if let Some(marker) = library_marker(&resolved) {
        bail!(
            "refusing to use {} as {purpose}: it resolves to {}, which is inside the Steam library at {}.\n\
             A Steam install is read-only reference, never written to, launched or injected into. \
             Fetch a server with `cargo xtask server fetch` and work in that copy.",
            path.display(),
            resolved.display(),
            marker.display()
        );
    }
    Ok(())
}

/// The Steam library covering `resolved`, if there is one.
///
/// Two shapes count. A `steamapps` component followed by `common` means the
/// path sits inside a library's content tree, which holds with no filesystem
/// behind the path. Otherwise an ancestor holding a library-shaped `steamapps`
/// directory means the same thing, which catches a library root named directly.
fn library_marker(resolved: &Path) -> Option<PathBuf> {
    if let Some(marker) = content_tree(resolved) {
        return Some(marker);
    }
    let mut walked = PathBuf::new();
    for component in resolved.components() {
        walked.push(component);
        if equals_ignoring_case(component.as_os_str(), STEAMAPPS) && is_library(&walked) {
            return Some(walked);
        }
    }
    for ancestor in resolved.ancestors() {
        let candidate = ancestor.join(STEAMAPPS);
        if is_library(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// The `steamapps` directory of a library whose `common` tree this path enters.
///
/// This is the one shape that needs no filesystem, so it is checked first and
/// keeps an unreachable network path from costing a lookup timeout.
fn content_tree(path: &Path) -> Option<PathBuf> {
    let components: Vec<&OsStr> = path.components().map(Component::as_os_str).collect();
    let mut walked = PathBuf::new();
    for (index, component) in components.iter().enumerate() {
        walked.push(component);
        if equals_ignoring_case(component, STEAMAPPS)
            && components
                .get(index + 1)
                .is_some_and(|name| equals_ignoring_case(name, "common"))
        {
            return Some(walked);
        }
    }
    None
}

/// Whether a `steamapps` directory is a library rather than an install target.
fn is_library(steamapps: &Path) -> bool {
    LIBRARY_MARKERS
        .iter()
        .any(|marker| steamapps.join(marker).exists())
}

/// Whether one path component matches a name, ignoring ASCII case.
fn equals_ignoring_case(name: &OsStr, wanted: &str) -> bool {
    name.to_str()
        .is_some_and(|text| text.eq_ignore_ascii_case(wanted))
}

/// Make `path` absolute and resolve every link along the part that exists.
///
/// A path naming a directory that is not there yet still resolves: the longest
/// existing prefix is canonicalized and the rest is appended, so a fetch target
/// under a junction is judged by where the junction points.
fn resolve(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut head = absolute.as_path();
    let mut tail: Vec<OsString> = Vec::new();
    loop {
        if let Ok(real) = std::fs::canonicalize(head) {
            let mut out = real;
            for part in tail.iter().rev() {
                out.push(part);
            }
            return out;
        }
        let (Some(parent), Some(name)) = (head.parent(), head.file_name()) else {
            return absolute;
        };
        tail.push(name.to_os_string());
        head = parent;
    }
}

#[cfg(test)]
mod tests {
    use super::{content_tree, ensure_outside_library};
    use crate::testutil::TestDir;
    use std::path::{Path, PathBuf};

    /// The `steamapps/common` pair settles a path on its own. Nothing here
    /// exists on disk, so only the component rule can produce these answers and
    /// a machine with a real Steam library cannot make the case pass for the
    /// wrong reason.
    #[test]
    fn the_content_tree_rule_reads_components_and_ignores_case() {
        let refused: &[&str] = &[
            r"D:\SteamLibrary\steamapps\common\Enshrouded",
            r"D:/SteamLibrary/steamapps/common/Enshrouded",
            r"D:\SteamLibrary\SteamApps\Common\Enshrouded",
            r"D:\SteamLibrary\STEAMAPPS\common\Enshrouded",
            r"D:\SteamLibrary\steamapps\common\Enshrouded\enshrouded.exe",
            r"C:\Program Files (x86)\Steam\steamapps\common\Enshrouded",
            r"\\fileserver\games\steamapps\common\Enshrouded",
            r"//fileserver/games/steamapps/common/Enshrouded",
        ];
        for case in refused {
            let marker = content_tree(Path::new(case))
                .unwrap_or_else(|| panic!("{case} is inside a Steam content tree"));
            assert!(
                marker
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.eq_ignore_ascii_case("steamapps")),
                "{case}: the marker must be the steamapps directory, got {}",
                marker.display()
            );
        }

        let accepted: &[&str] = &[
            r"Z:\repos\ember\.cache\server\23178631",
            r"Z:\repos\ember\.cache\server\23178631\steamapps",
            r"D:\SteamLibrary\steamapps",
            r"D:\SteamLibrary\steamappscommon\Enshrouded",
            r"D:\backups\steamapps-common\Enshrouded",
            r"D:\common\steamapps",
        ];
        for case in accepted {
            assert!(
                content_tree(Path::new(case)).is_none(),
                "{case} is not inside a Steam content tree"
            );
        }
    }

    /// The refusal names the library and the way forward. The path is under the
    /// sandbox and is never created, so the answer comes from the component
    /// rule alone.
    #[test]
    fn a_refusal_names_the_library_and_the_way_forward() {
        let dir = TestDir::new("steam-message");
        let case = dir
            .path()
            .join("SteamLibrary")
            .join("steamapps")
            .join("common")
            .join("Enshrouded");
        let err = ensure_outside_library(&case, "a server directory")
            .unwrap_err_or_panic("a path inside a content tree");
        assert!(
            err.contains("Steam library"),
            "the refusal must name the Steam library, got {err}"
        );
        assert!(
            err.contains("server fetch"),
            "the refusal must name the way forward, got {err}"
        );
        assert!(
            err.contains("steamapps"),
            "the refusal must name the directory that matched, got {err}"
        );
    }

    /// Paths with no Steam library above them pass.
    #[test]
    fn ordinary_paths_are_accepted() {
        let dir = TestDir::new("steam-ordinary");
        let cases: Vec<PathBuf> = vec![
            dir.path().join("server").join("23178631"),
            dir.path().join("steamapps-archive"),
            dir.path().join("my steamapps notes"),
            dir.path().join("SteamLibraryBackup"),
        ];
        for case in cases {
            ensure_outside_library(&case, "a server directory")
                .unwrap_or_else(|err| panic!("{} must be accepted: {err}", case.display()));
        }
    }

    /// A `SteamCMD` install directory carries a `steamapps` directory holding
    /// only app manifests. Refusing it would refuse every build this loop
    /// fetches, so the narrower library shape is what the rule matches.
    #[test]
    fn a_steamcmd_install_directory_is_accepted() {
        let dir = TestDir::new("steam-install");
        let build = dir.path().join("server").join("23178631");
        dir.write(
            "server/23178631/steamapps/appmanifest_2278520.acf",
            b"\"AppState\" { \"buildid\" \"23178631\" }",
        );
        dir.write("server/23178631/enshrouded_server.exe", b"MZ");
        ensure_outside_library(&build, "a server directory")
            .expect("a fetched build is not a Steam library");
        ensure_outside_library(dir.path(), "the dev root")
            .expect("a cache holding a fetched build is not a Steam library");
    }

    /// A directory holding a library-shaped `steamapps` child is a library
    /// root, even when the caller names the root rather than anything under it.
    #[test]
    fn a_library_root_is_refused_by_its_steamapps_child() {
        for marker in ["common", "libraryfolders.vdf"] {
            let dir = TestDir::new("steam-root");
            dir.write(&format!("SteamLibrary/steamapps/{marker}/keep"), b"");
            let library = dir.path().join("SteamLibrary");
            ensure_outside_library(&library, "a server directory")
                .unwrap_err_or_panic(&format!("library marked by {marker}"));
            ensure_outside_library(&library.join("steamapps"), "a server directory")
                .unwrap_err_or_panic(&format!("steamapps marked by {marker}"));
        }
    }

    /// A relative path is judged by where it resolves, not by how it is typed.
    #[test]
    fn a_relative_path_resolving_inside_a_library_is_refused() {
        let dir = TestDir::new("steam-relative");
        let deep = dir
            .path()
            .join("SteamLibrary")
            .join("steamapps")
            .join("common")
            .join("Enshrouded");
        std::fs::create_dir_all(&deep).unwrap();
        let relative = deep.join("..").join("..").join("common").join("Enshrouded");
        ensure_outside_library(&relative, "a server directory")
            .unwrap_err_or_panic("a relative path landing in a library");
    }

    /// A link is judged by its target. Creating one needs developer mode on
    /// Windows, so the case reports itself as skipped rather than failing.
    #[test]
    fn a_link_into_a_library_is_refused() {
        let dir = TestDir::new("steam-link");
        let real = dir
            .path()
            .join("SteamLibrary")
            .join("steamapps")
            .join("common")
            .join("Enshrouded");
        std::fs::create_dir_all(&real).unwrap();
        let link = dir.path().join("server-link");
        if !TestDir::link_dir(&real, &link) {
            eprintln!("skipped: this machine does not allow creating a directory link");
            return;
        }
        ensure_outside_library(&link, "a server directory").unwrap_err_or_panic("a link");
        ensure_outside_library(&link.join("enshrouded_server.exe"), "a server directory")
            .unwrap_err_or_panic("a path under a link");
    }

    /// Turn a refusal into its message, or fail the case by name.
    trait ExpectRefusal {
        fn unwrap_err_or_panic(self, case: &str) -> String;
    }

    impl ExpectRefusal for anyhow::Result<()> {
        fn unwrap_err_or_panic(self, case: &str) -> String {
            match self {
                Ok(()) => panic!("{case} must be refused"),
                Err(err) => format!("{err}"),
            }
        }
    }
}
