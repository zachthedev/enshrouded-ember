//! What each row that walks the tree proves it checked.
//!
//! A tool that exits zero may have read nothing: taplo passes over a file its
//! config excludes, Prettier skips an ignored one without a word, and
//! actionlint lints only what it is handed. Each such row hands its tool an
//! explicit list of tracked files, reads back which ones the tool reports
//! finishing, and fails when the two differ or when the list is empty. The
//! row's note is the count.

use std::collections::BTreeSet;
use std::path::Path;

/// How many characters of file arguments one command carries. Windows caps a
/// whole command line at 32,767, so a longer list runs in batches.
const ARGUMENT_BUDGET: usize = 24_000;

/// The prefix every reusable workflow a job may hand its secrets to carries.
pub const SHARED_WORKFLOWS: &str = "zachthedev/.github/.github/workflows/";

/// `paths` in batches that fit [`ARGUMENT_BUDGET`].
#[must_use]
pub fn batches(paths: &[String]) -> Vec<Vec<String>> {
    let mut all = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut length = 0;
    for path in paths {
        if !current.is_empty() && length + path.len() + 1 > ARGUMENT_BUDGET {
            all.push(std::mem::take(&mut current));
            length = 0;
        }
        length += path.len() + 1;
        current.push(path.clone());
    }
    if !current.is_empty() {
        all.push(current);
    }
    all
}

/// How a count reads in a row's note: `1 file`, `9 files`.
#[must_use]
pub fn count(number: usize, one: &str, many: &str) -> String {
    format!("{number} {}", if number == 1 { one } else { many })
}

/// `printed`, a path a tool wrote, relative to `root` with `/` separators, and
/// in lower case where the filesystem ignores case, so two spellings of one
/// file compare equal.
///
/// A path under `root` loses the root; any other path is kept whole. The
/// Windows verbatim prefix rustfmt prints is dropped first.
#[must_use]
pub fn comparable(printed: &str, root: &Path) -> String {
    let normalize = |path: &str| {
        let path = path.trim();
        let path = path.strip_prefix(r"\\?\").unwrap_or(path);
        let path = path.replace('\\', "/");
        let path = path.strip_prefix("./").unwrap_or(&path).to_string();
        if cfg!(any(windows, target_os = "macos")) {
            path.to_lowercase()
        } else {
            path
        }
    };
    let path = normalize(printed);
    let root = normalize(&root.display().to_string());
    let root = root.trim_end_matches('/');
    path.strip_prefix(root)
        .and_then(|rest| rest.strip_prefix('/'))
        .map_or(path.clone(), str::to_string)
}

/// What a row hands its tool, as a finding names it: one, then many.
pub type Noun = [&'static str; 2];

/// Files, which every row but machete hands over.
pub const FILES: Noun = ["file", "files"];

/// Crate directories, which the machete row hands over.
pub const CRATES: Noun = ["crate", "crates"];

/// Whether every path in `handed` is among the `reported` ones and nothing else
/// is, both read through [`comparable`], as the sentence a failing row carries.
///
/// # Errors
///
/// Returns the sentence naming what the tool never reported, or the tool's
/// report of a path nobody handed it, or that nothing was handed at all.
pub fn prove(
    tool: &str,
    noun: Noun,
    handed: &[String],
    reported: &[String],
    root: &Path,
) -> Result<(), String> {
    covered(tool, noun, handed, reported, root)?;
    let wanted: BTreeSet<String> = handed.iter().map(|path| comparable(path, root)).collect();
    let extra: Vec<&String> = reported
        .iter()
        .filter(|path| !wanted.contains(&comparable(path, root)))
        .collect();
    if !extra.is_empty() {
        return Err(format!(
            "{tool} reported {}, which the row never handed it",
            join(&extra)
        ));
    }
    Ok(())
}

/// Whether every path in `handed` is among the `reported` ones, both read
/// through [`comparable`], for a tool that finds its own files and may report
/// more than the tracked ones, as the sentence a failing row carries.
///
/// # Errors
///
/// Returns the sentence naming, as handed, what the tool never reported, or
/// that nothing was handed at all.
pub fn covered(
    tool: &str,
    noun: Noun,
    handed: &[String],
    reported: &[String],
    root: &Path,
) -> Result<(), String> {
    let [one, many] = noun;
    if handed.is_empty() {
        return Err(format!(
            "no tracked {one} is one {tool} reads, so the row checks nothing"
        ));
    }
    let got: BTreeSet<String> = reported.iter().map(|path| comparable(path, root)).collect();
    let missed: Vec<&String> = handed
        .iter()
        .filter(|path| !got.contains(&comparable(path, root)))
        .collect();
    if missed.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{tool} never reported {} of the {} it had to read: {}",
            count(missed.len(), one, many),
            count(handed.len(), one, many),
            join(&missed)
        ))
    }
}

/// `paths` quoted and joined for a finding.
fn join(paths: &[&String]) -> String {
    paths
        .iter()
        .map(|path| format!("{path:?}"))
        .collect::<Vec<_>>()
        .join(", ")
}

// ///////////////////////////////////////////////
// What each tool reports
// ///////////////////////////////////////////////

/// Every file rustfmt's `--verbose` output names as formatted.
#[must_use]
pub fn rustfmt_formatted(printed: &str) -> Vec<String> {
    printed
        .lines()
        .filter_map(|line| line.strip_prefix("Formatting "))
        .map(str::to_string)
        .collect()
}

/// Every path in taplo's `found files ... files=[...]` log line, or `None` when
/// it printed none. taplo prints the line at `RUST_LOG=info`.
#[must_use]
pub fn taplo_found(printed: &str) -> Option<Vec<String>> {
    let line = printed
        .lines()
        .find(|line| line.contains("found files total="))?;
    let list = line.split_once("files=[")?.1;
    let list = &list[..list.find(']')?];
    let mut paths = Vec::new();
    let mut rest = list;
    while let Some(start) = rest.find('"') {
        let after = &rest[start + 1..];
        let mut end = None;
        let mut escaped = false;
        for (at, character) in after.char_indices() {
            match character {
                '\\' if !escaped => escaped = true,
                '"' if !escaped => {
                    end = Some(at);
                    break;
                }
                _ => escaped = false,
            }
        }
        let end = end?;
        paths.push(after[..end].replace("\\\\", "\\").replace("\\\"", "\""));
        rest = &after[end + 1..];
    }
    Some(paths)
}

/// Every file actionlint's `-verbose` output names as linted, from its
/// `Found total N errors in M ms for <path>` lines.
#[must_use]
pub fn actionlint_linted(printed: &str) -> Vec<String> {
    printed
        .lines()
        .filter_map(|line| {
            let line = line.trim_start_matches("verbose: ");
            line.strip_prefix("Found total ")?;
            Some(line.rsplit_once(" ms for ")?.1.trim_end().to_string())
        })
        .collect()
}

/// Every input zizmor's log names as completed, printed at `RUST_LOG=info`.
#[must_use]
pub fn zizmor_completed(printed: &str) -> Vec<String> {
    printed
        .lines()
        .filter_map(|line| Some(line.split_once("completed ")?.1.trim_end().to_string()))
        .collect()
}

/// Every directory cargo-machete names as analyzed, whether it found an unused
/// dependency there or not.
#[must_use]
pub fn machete_analyzed(printed: &str) -> Vec<String> {
    printed
        .lines()
        .filter_map(|line| {
            line.strip_prefix("cargo-machete didn't find any unused dependencies in ")
                .and_then(|rest| rest.strip_suffix(". Good job!"))
                .or_else(|| {
                    line.strip_prefix("cargo-machete found the following unused dependencies in ")
                        .and_then(|rest| rest.strip_suffix(':'))
                })
                .map(str::to_string)
        })
        .collect()
}

/// Every job zizmor's JSON report names as passing `secrets: inherit` to a
/// workflow outside [`SHARED_WORKFLOWS`], as one sentence each.
///
/// The report comes from a run with `--no-config --no-ignores`, so no waiver in
/// `zizmor.yml` or inline hides a job from it. The called workflow is the
/// primary location's feature.
///
/// # Errors
///
/// Returns the sentence saying the report does not read as zizmor's JSON.
pub fn inherit_callees(report: &str) -> Result<Vec<String>, String> {
    let findings: serde_json::Value = serde_json::from_str(report)
        .map_err(|err| format!("zizmor's JSON report does not parse: {err}"))?;
    let findings = findings
        .as_array()
        .ok_or("zizmor's JSON report is not a list of findings")?;
    let mut found = Vec::new();
    for finding in findings {
        if finding.get("ident").and_then(serde_json::Value::as_str) != Some("secrets-inherit") {
            continue;
        }
        let locations = finding
            .get("locations")
            .and_then(serde_json::Value::as_array)
            .ok_or("a secrets-inherit finding carries no locations")?;
        let primary = locations
            .iter()
            .find(|location| {
                location
                    .pointer("/symbolic/kind")
                    .and_then(serde_json::Value::as_str)
                    == Some("Primary")
            })
            .ok_or("a secrets-inherit finding carries no primary location")?;
        let file = primary
            .pointer("/symbolic/key/Local/given_path")
            .or_else(|| primary.pointer("/symbolic/key/Local/verbatim_path"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("a workflow");
        let callee = primary
            .pointer("/concrete/feature")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        if !callee.trim().starts_with(SHARED_WORKFLOWS) {
            found.push(format!(
                "{file:?} passes secrets: inherit to {:?}, and a job may hand its secrets only to a workflow under {SHARED_WORKFLOWS}",
                callee.trim()
            ));
        }
    }
    Ok(found)
}

/// The script Bun runs to ask Prettier which of the paths on standard input it
/// formats: not ignored by `.prettierignore`, with a parser it infers.
///
/// `resolveConfig: false` keeps `getFileInfo` from loading the nearest config,
/// a `package.json` `prettier` key among them, inside this process.
pub const PRETTIER_FILES: &str = "\
const { getFileInfo } = await import('./node_modules/prettier/index.mjs');
const paths = (await Bun.stdin.text()).split('\\0').filter((path) => path.length > 0);
const kept = [];
for (const path of paths) {
  const info = await getFileInfo(path, { ignorePath: '.prettierignore', resolveConfig: false });
  if (!info.ignored && info.inferredParser !== null) {
    kept.push(path);
  }
}
process.stdout.write(kept.join('\\0'));
";

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        FILES, SHARED_WORKFLOWS, actionlint_linted, batches, comparable, count, covered,
        inherit_callees, machete_analyzed, prove, rustfmt_formatted, taplo_found, zizmor_completed,
    };

    fn owned(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|path| (*path).to_string()).collect()
    }

    /// A printed path compares against a handed one whatever its separators,
    /// its verbatim prefix or its `./`, relative to the root, and a path
    /// outside the root is kept whole.
    #[test]
    fn a_printed_path_compares_relative_to_the_root() {
        let root = Path::new("/work/repo");
        for (printed, wanted) in [
            ("/work/repo/crates/a/src/lib.rs", "crates/a/src/lib.rs"),
            ("./crates/a/Cargo.toml", "crates/a/Cargo.toml"),
            ("crates/a", "crates/a"),
            ("/elsewhere/lib.rs", "/elsewhere/lib.rs"),
            ("/work/repository/x.rs", "/work/repository/x.rs"),
        ] {
            let wanted = if cfg!(any(windows, target_os = "macos")) {
                wanted.to_lowercase()
            } else {
                wanted.to_string()
            };
            assert_eq!(comparable(printed, root), wanted, "{printed}");
        }
        let root = Path::new(r"Z:\repos\mods");
        let printed = r"\\?\Z:\repos\mods\crates\a\src\lib.rs";
        assert_eq!(comparable(printed, root), "crates/a/src/lib.rs");
    }

    /// Every handed file must be reported, and a report of a file nobody
    /// handed over fails the exact proof alone.
    #[test]
    fn a_proof_names_what_the_tool_missed_or_added() {
        let root = Path::new("/r");
        let handed = owned(&["a.toml", "b.toml"]);
        assert_eq!(
            prove(
                "taplo",
                FILES,
                &handed,
                &owned(&["/r/a.toml", "/r/b.toml"]),
                root
            ),
            Ok(())
        );
        assert_eq!(
            prove("taplo", FILES, &handed, &owned(&["/r/a.toml"]), root),
            Err(
                "taplo never reported 1 file of the 2 files it had to read: \"b.toml\"".to_string()
            )
        );
        assert_eq!(
            prove(
                "taplo",
                FILES,
                &handed,
                &owned(&["a.toml", "b.toml", "c.toml"]),
                root
            ),
            Err("taplo reported \"c.toml\", which the row never handed it".to_string())
        );
        assert_eq!(
            prove("taplo", FILES, &[], &[], root),
            Err("no tracked file is one taplo reads, so the row checks nothing".to_string())
        );
        assert_eq!(
            covered(
                "rustfmt",
                FILES,
                &handed,
                &owned(&["a.toml", "b.toml", "c.toml"]),
                root
            ),
            Ok(())
        );
        assert_eq!(
            covered("rustfmt", FILES, &handed, &owned(&["a.toml"]), root),
            Err(
                "rustfmt never reported 1 file of the 2 files it had to read: \"b.toml\""
                    .to_string()
            )
        );
    }

    /// Each parser reads the lines the pinned tool prints, as measured on
    /// each, and nothing else.
    #[test]
    fn each_tool_report_reads_as_measured() {
        let rustfmt = "Using rustfmt config file \\\\?\\Z:\\r\\rustfmt.toml\nFormatting \\\\?\\Z:\\r\\a\\src\\lib.rs\nSpent 0.001 secs in the parsing phase\nFormatting \\\\?\\Z:\\r\\b.rs\n";
        assert_eq!(
            rustfmt_formatted(rustfmt),
            owned(&[r"\\?\Z:\r\a\src\lib.rs", r"\\?\Z:\r\b.rs"])
        );

        let taplo = " INFO taplo:format_files:collect_files: found files total=2 excluded=0 files=[\"Z:/r/Cargo.toml\", \"Z:/r/a \\\"q\\\".toml\"] cwd=\"Z:/r\"\n";
        assert_eq!(
            taplo_found(taplo),
            Some(owned(&["Z:/r/Cargo.toml", "Z:/r/a \"q\".toml"]))
        );
        assert_eq!(taplo_found(" INFO taplo: formatted nothing\n"), None);

        let actionlint = "verbose: Found total 0 errors in 2 ms for .github/workflows/deps.yml\nverbose: Found total 1 error in 145 ms for .github/workflows/ci.yml\nverbose: linting 2 files\n";
        assert_eq!(
            actionlint_linted(actionlint),
            owned(&[".github/workflows/deps.yml", ".github/workflows/ci.yml"])
        );

        let zizmor = " INFO audit: zizmor: \u{1F308} completed .github\\workflows\\ci.yml\n INFO audit: zizmor: \u{1F308} completed .github\\dependabot.yml\n INFO zizmor: collecting inputs\n";
        assert_eq!(
            zizmor_completed(zizmor),
            owned(&[r".github\workflows\ci.yml", r".github\dependabot.yml"])
        );

        let machete = "Analyzing dependencies of crates in a,b...\ncargo-machete found the following unused dependencies in a:\na -- a\\Cargo.toml:\n\tb\ncargo-machete didn't find any unused dependencies in ./b. Good job!\nDone!\n";
        assert_eq!(machete_analyzed(machete), owned(&["a", "./b"]));
    }

    /// A job may hand its secrets only to a workflow under the shared prefix,
    /// read from the primary location's feature, and a report that does not
    /// read as zizmor's JSON is refused rather than read as clean.
    #[test]
    fn a_secrets_inherit_callee_is_held_to_the_shared_workflows() {
        let finding = |feature: &str| {
            format!(
                r#"[{{"ident":"secrets-inherit","locations":[{{"symbolic":{{"kind":"Related","key":{{"Local":{{"verbatim_path":"x"}}}}}},"concrete":{{"feature":"secrets: inherit"}}}},{{"symbolic":{{"kind":"Primary","key":{{"Local":{{"verbatim_path":".github/workflows/deps.yml"}}}}}},"concrete":{{"feature":"{feature}"}}}}]}}]"#
            )
        };
        let shared = format!("{SHARED_WORKFLOWS}deps.yml@c53d09e393028ceddee0d761f2a7963394289a72");
        assert_eq!(inherit_callees(&finding(&shared)), Ok(Vec::new()));
        assert_eq!(inherit_callees("[]"), Ok(Vec::new()));
        assert_eq!(
            inherit_callees(&finding("evil/actions/.github/workflows/x.yml@main")),
            Ok(vec![
                "\".github/workflows/deps.yml\" passes secrets: inherit to \"evil/actions/.github/workflows/x.yml@main\", and a job may hand its secrets only to a workflow under zachthedev/.github/.github/workflows/".to_string()
            ])
        );
        assert_eq!(
            inherit_callees(&finding(
                "zachthedev/.github-evil/.github/workflows/x.yml@main"
            ))
            .map(|found| found.len()),
            Ok(1),
            "a name the owner's repository starts with is another repository"
        );
        let other = r#"[{"ident":"unpinned-uses","locations":[]}]"#;
        assert_eq!(inherit_callees(other), Ok(Vec::new()));
        assert!(inherit_callees("not json").is_err());
        assert!(inherit_callees(r#"{"ident":"secrets-inherit"}"#).is_err());
        assert!(inherit_callees(r#"[{"ident":"secrets-inherit","locations":[]}]"#).is_err());
    }

    /// A list longer than one command line holds splits into batches that each
    /// fit, keeping every path once and in order.
    #[test]
    fn a_long_list_runs_in_batches() {
        let paths: Vec<String> = (0..3000)
            .map(|n| format!("crates/a/src/module_{n:05}.rs"))
            .collect();
        let split = batches(&paths);
        assert!(split.len() > 1, "{} paths fit one batch", paths.len());
        assert!(
            split
                .iter()
                .all(|batch| { batch.iter().map(|path| path.len() + 1).sum::<usize>() <= 24_000 })
        );
        assert_eq!(split.concat(), paths);
        assert_eq!(batches(&[]), Vec::<Vec<String>>::new());
        assert_eq!(count(1, "file", "files"), "1 file");
        assert_eq!(count(9, "file", "files"), "9 files");
    }
}
