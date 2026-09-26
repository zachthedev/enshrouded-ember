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

/// The documented examples one `cargo test --doc` run counted, summed over the
/// result line each crate's `Doc-tests` header opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Examples {
    /// Examples that ran and passed, a `no_run` one compiled alone included.
    pub passed: usize,
    /// Examples marked `ignore`.
    pub ignored: usize,
    /// Examples a test filter left out.
    pub filtered: usize,
}

/// The examples `printed`, what `cargo test --doc` wrote, counts, or why the
/// count cannot be read.
///
/// cargo exits zero on a crate with no examples, on one whose every example is
/// ignored or filtered out, and on `--list`, which prints no result line, so
/// only the count says what ran. A crate holding a `compile_fail` or
/// `standalone_crate` example prints a second result line for its separate
/// pass, so every header needs at least one and every result line counts.
///
/// # Errors
///
/// Returns the sentence the doctests row carries when a `Doc-tests` header has
/// no result line, or a result line lacks a count.
pub fn doctest_examples(printed: &str) -> Result<Examples, String> {
    let headers = printed
        .lines()
        .filter(|line| line.trim_start().starts_with("Doc-tests "))
        .count();
    let results: Vec<&str> = printed
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("test result: "))
        .collect();
    if results.len() < headers {
        return Err(format!(
            "cargo test --doc printed {} and {}, so which examples ran is unknown",
            count(headers, "Doc-tests header", "Doc-tests headers"),
            count(results.len(), "result line", "result lines")
        ));
    }
    let mut examples = Examples::default();
    for result in results {
        let field = |name: &str| {
            result
                .split(';')
                .find_map(|part| {
                    part.trim()
                        .strip_suffix(name)?
                        .trim()
                        .rsplit(' ')
                        .next()?
                        .parse::<usize>()
                        .ok()
                })
                .ok_or_else(|| {
                    format!(
                        "cargo test --doc printed a result line with no {} count: {result}",
                        name.trim()
                    )
                })
        };
        examples.passed += field(" passed")?;
        examples.ignored += field(" ignored")?;
        examples.filtered += field(" filtered out")?;
    }
    Ok(examples)
}

/// The doctests row's note for `examples`, or the sentence saying why they
/// prove nothing ran. `declared_none` is a repository's declaration that it
/// holds no documented example, which holds only while none is counted.
///
/// # Errors
///
/// Returns the sentence the doctests row carries when an example was filtered
/// out, when every counted example was ignored, when none was counted and none
/// is declared, or when one was counted though none is declared.
pub fn doctests_proven(examples: Examples, declared_none: bool) -> Result<String, String> {
    let counted = examples.passed + examples.ignored + examples.filtered;
    if examples.filtered > 0 {
        return Err(format!(
            "cargo test --doc filtered out {}, so the row did not run them all",
            count(examples.filtered, "example", "examples")
        ));
    }
    if declared_none {
        return if counted == 0 {
            Ok("no examples, as declared".to_string())
        } else {
            Err(format!(
                "the gate declares no documented example, and cargo test --doc counted {counted}. Remove the declaration"
            ))
        };
    }
    if counted == 0 {
        return Err(
            "cargo test --doc ran no documented example. Write one, or declare none beside the step table"
                .to_string(),
        );
    }
    if examples.passed == 0 {
        return Err(format!(
            "cargo test --doc ignored every example it counted, {counted} of them, so none ran"
        ));
    }
    Ok(count(examples.passed, "example", "examples"))
}

/// How many tests nextest's one summary line reports it skipped. nextest
/// counts an `#[ignore]` test and a filtered one alike as skipped.
///
/// # Errors
///
/// Returns the sentence saying the output holds no single summary line, or a
/// summary with no skipped count.
pub fn nextest_skipped(printed: &str) -> Result<usize, String> {
    let summaries: Vec<&str> = printed
        .lines()
        .filter(|line| line.trim_start().starts_with("Summary ["))
        .collect();
    let [summary] = summaries.as_slice() else {
        return Err(format!(
            "nextest printed {}, and the tests row reads exactly one",
            count(summaries.len(), "summary line", "summary lines")
        ));
    };
    let totals = summary.split_once(']').map_or("", |(_, rest)| rest);
    totals
        .split([',', ':'])
        .find_map(|part| part.trim().strip_suffix(" skipped"))
        .and_then(|number| number.trim().parse().ok())
        .ok_or_else(|| {
            format!(
                "nextest's summary names no skipped count: {}",
                summary.trim()
            )
        })
}

/// The note a test row carries when `skipped`, the tests its runner skipped,
/// equals `declared`, the count the gate declares beside the step table, or
/// the sentence saying how they differ.
///
/// # Errors
///
/// Returns the sentence naming both counts when they differ, so a change that
/// skips a test or stops skipping one declares it in the same diff.
pub fn skips_proven(runner: &str, skipped: usize, declared: usize) -> Result<String, String> {
    if skipped == declared {
        Ok(format!("{skipped} skipped, as declared"))
    } else {
        Err(format!(
            "{runner} skipped {}, and the gate declares {declared}. Change the declaration beside the step table in the same change as the tests",
            count(skipped, "test", "tests")
        ))
    }
}

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
        Examples, FILES, actionlint_linted, batches, comparable, count, covered, doctest_examples,
        doctests_proven, machete_analyzed, nextest_skipped, prove, rustfmt_formatted, skips_proven,
        taplo_found, zizmor_completed,
    };

    /// The skipped count comes from nextest's one summary line, whatever else
    /// it counts beside it, and a missing or doubled summary is refused.
    #[test]
    fn nextest_skips_are_read_from_its_summary() {
        for (what, printed, wanted) in [
            (
                "none skipped",
                "   PASS [   0.143s] (1/1) a\n────────────\n     Summary [   2.594s] 152 tests run: 152 passed, 0 skipped\n",
                Ok(0),
            ),
            (
                "some skipped",
                "     Summary [   3.256s] 267 tests run: 267 passed, 12 skipped",
                Ok(12),
            ),
            (
                "failures beside the skips",
                "     Summary [   1.000s] 5 tests run: 3 passed, 1 failed, 1 timed out, 2 skipped",
                Ok(2),
            ),
            (
                "no summary",
                "   PASS [   0.143s] (1/1) a",
                Err("nextest printed 0 summary lines, and the tests row reads exactly one".to_string()),
            ),
            (
                "two summaries",
                "Summary [ 1s] 1 tests run: 1 passed, 0 skipped\nSummary [ 1s] 1 tests run: 1 passed, 0 skipped",
                Err("nextest printed 2 summary lines, and the tests row reads exactly one".to_string()),
            ),
            (
                "a summary with no skipped count",
                "     Summary [   1.000s] 1 tests run: 1 passed",
                Err("nextest's summary names no skipped count: Summary [   1.000s] 1 tests run: 1 passed".to_string()),
            ),
        ] {
            assert_eq!(nextest_skipped(printed), wanted, "{what}");
        }
    }

    /// A skip count passes only when it equals the declared one.
    #[test]
    fn skips_pass_only_at_the_declared_count() {
        assert_eq!(
            skips_proven("nextest", 12, 12),
            Ok("12 skipped, as declared".to_string())
        );
        assert_eq!(
            skips_proven("nextest", 13, 12),
            Err("nextest skipped 13 tests, and the gate declares 12. Change the declaration beside the step table in the same change as the tests".to_string()),
            "one more"
        );
        assert_eq!(
            skips_proven("bun test", 0, 1),
            Err("bun test skipped 0 tests, and the gate declares 1. Change the declaration beside the step table in the same change as the tests".to_string()),
            "one fewer"
        );
    }

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
        let root = Path::new(r"C:\work\mods");
        let printed = r"\\?\C:\work\mods\crates\a\src\lib.rs";
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

    /// One crate's `cargo test --doc` block as the pinned cargo prints it.
    fn block(name: &str, passed: usize, ignored: usize, filtered: usize) -> String {
        format!(
            "   Doc-tests {name}\n\nrunning {}\n\ntest result: ok. {passed} passed; 0 failed; {ignored} ignored; 0 measured; {filtered} filtered out; finished in 0.00s\n\n",
            passed + ignored
        )
    }

    /// The examples sum over every crate's result line, and a header with no
    /// result line after it, as `--list` prints, leaves the count unknown.
    #[test]
    fn a_doctest_run_counts_every_crates_examples() {
        let examples = |passed, ignored, filtered| Examples {
            passed,
            ignored,
            filtered,
        };
        for (what, printed, wanted) in [
            (
                "two crates",
                format!("{}{}", block("a", 1, 0, 0), block("b", 3, 1, 0)),
                examples(4, 1, 0),
            ),
            ("a crate with none", block("a", 0, 0, 0), examples(0, 0, 0)),
            ("a filtered run", block("a", 0, 0, 2), examples(0, 0, 2)),
            ("no library at all", String::new(), examples(0, 0, 0)),
            (
                "a crate with a separate pass",
                format!(
                    "{}test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n",
                    block("a", 2, 0, 0)
                ),
                examples(3, 0, 0),
            ),
        ] {
            assert_eq!(doctest_examples(&printed), Ok(wanted), "{what}");
        }
        assert_eq!(
            doctest_examples("   Doc-tests a\nsrc/lib.rs - add (line 3): test\n\n1 test, 0 benchmarks\n"),
            Err("cargo test --doc printed 1 Doc-tests header and 0 result lines, so which examples ran is unknown".to_string()),
            "a listed run"
        );
        assert_eq!(
            doctest_examples("   Doc-tests a\ntest result: ok. 1 passed; 0 failed; 0 measured\n"),
            Err("cargo test --doc printed a result line with no ignored count: ok. 1 passed; 0 failed; 0 measured".to_string()),
            "a result line missing a count"
        );
    }

    /// The row passes on a run that ran an example, or on none where none is
    /// declared, and fails on a filtered run, on every example ignored, on none
    /// undeclared and on one counted though none is declared.
    #[test]
    fn a_doctest_count_proves_what_ran() {
        let examples = |passed, ignored, filtered| Examples {
            passed,
            ignored,
            filtered,
        };
        for (what, counted, declared, wanted) in [
            ("four ran", examples(4, 0, 0), false, Ok("4 examples")),
            (
                "one ran beside an ignored one",
                examples(1, 1, 0),
                false,
                Ok("1 example"),
            ),
            (
                "none, declared",
                examples(0, 0, 0),
                true,
                Ok("no examples, as declared"),
            ),
            (
                "none, undeclared",
                examples(0, 0, 0),
                false,
                Err(
                    "cargo test --doc ran no documented example. Write one, or declare none beside the step table",
                ),
            ),
            (
                "every one ignored",
                examples(0, 2, 0),
                false,
                Err("cargo test --doc ignored every example it counted, 2 of them, so none ran"),
            ),
            (
                "one filtered out",
                examples(3, 0, 1),
                false,
                Err("cargo test --doc filtered out 1 example, so the row did not run them all"),
            ),
            (
                "filtered out where none is declared",
                examples(0, 0, 2),
                true,
                Err("cargo test --doc filtered out 2 examples, so the row did not run them all"),
            ),
            (
                "one counted though none is declared",
                examples(1, 0, 0),
                true,
                Err(
                    "the gate declares no documented example, and cargo test --doc counted 1. Remove the declaration",
                ),
            ),
        ] {
            assert_eq!(
                doctests_proven(counted, declared),
                wanted.map(str::to_string).map_err(str::to_string),
                "{what}"
            );
        }
    }
}
