//! The program actionlint runs in place of `ShellCheck`, and the shells a
//! workflow step may run under.
//!
//! actionlint hands each `run:` script to the program its `-shellcheck` flag
//! names, as the decoded text `ShellCheck` reads, on standard input. A directive
//! in that text drops findings, and YAML escapes, folding and a directive with
//! several keys hide one from any reading of the workflow file. The gate names
//! this binary there under a hidden subcommand, which refuses every line holding
//! a directive and otherwise runs the pinned `ShellCheck` over the same bytes.
//!
//! actionlint hands a script to `ShellCheck` only when its step runs under bash
//! or sh, so a step under any other shell is never analyzed. The actionlint row
//! reads every step's shell through Bun's YAML parser and refuses one outside
//! [`SHELLS`].

use std::io::{self, Read, Write};
use std::path::Path;
use std::process::{ExitCode, Stdio};

use crate::spawn;

/// The hidden subcommand the stand-in runs under.
pub const SUBCOMMAND: &str = "shellcheck-stand-in";

/// The sentence every refusal opens with, which the directive canary looks for.
pub const REFUSAL: &str = "A ShellCheck directive in a workflow script is refused";

/// The exit code for a failure of the stand-in's own. With nothing on standard
/// output, actionlint reports the run as failed rather than passing it unread.
const FAILED: u8 = 2;

/// The shells a workflow step may run under: the two actionlint hands to
/// `ShellCheck`, and pwsh.
pub const SHELLS: &[&str] = &["bash", "sh", "pwsh"];

/// The value for actionlint's `-shellcheck` flag: `stand_in`, the hidden
/// subcommand and `shellcheck`, each path single-quoted with forward slashes.
///
/// actionlint splits the value into words the way a shell does, where an
/// unquoted backslash is an escape, and a program it cannot find turns the
/// analysis off with nothing said.
///
/// # Errors
///
/// Returns the sentence a result row carries when a path holds a single quote,
/// which the split cannot carry.
pub fn flag_value(stand_in: &Path, shellcheck: &Path) -> Result<String, String> {
    let quoted = |path: &Path| {
        let text = path.display().to_string().replace('\\', "/");
        if text.contains('\'') {
            Err(format!(
                "{text:?} holds a single quote, and actionlint's word split cannot carry one"
            ))
        } else {
            Ok(format!("'{text}'"))
        }
    };
    Ok(format!(
        "{} {SUBCOMMAND} {}",
        quoted(stand_in)?,
        quoted(shellcheck)?
    ))
}

/// Whether `line` holds a `ShellCheck` directive: `#`, any spacing, the word
/// `shellcheck` in any case, then a space.
///
/// Spacing is every space `ShellCheck` accepts there, U+200B among them, which
/// `char::is_whitespace` leaves out. The case fold only widens the match.
fn directive(line: &str) -> bool {
    let spacing = |character: char| character.is_whitespace() || character == '\u{200B}';
    line.match_indices('#').any(|(at, _)| {
        let rest = line[at + 1..].trim_start_matches(spacing);
        rest.get(..10)
            .is_some_and(|word| word.eq_ignore_ascii_case("shellcheck"))
            && rest[10..].chars().next().is_some_and(spacing)
    })
}

/// One finding per line of `script` that holds a directive, in `ShellCheck`'s
/// JSON shape, with lines counted from one as `ShellCheck` counts them.
fn refusals(script: &str) -> Vec<serde_json::Value> {
    script
        .split('\n')
        .enumerate()
        .filter(|(_, line)| directive(line))
        .map(|(index, line)| {
            serde_json::json!({
                "file": "-",
                "line": index + 1,
                "endLine": index + 1,
                "column": 1,
                "endColumn": 1,
                "level": "error",
                "code": 0,
                "message": format!("{REFUSAL}, because it drops findings from actionlint's report: {:?}", line.trim()),
                "fix": null,
            })
        })
        .collect()
}

/// Run as the stand-in, where `args` is the `ShellCheck` path and then the
/// arguments actionlint passes.
///
/// A directive in the script on standard input comes back as findings. Any
/// other script goes to `ShellCheck` unchanged, with `SHELLCHECK_OPTS` removed,
/// and its output and exit code pass through. A failure of the stand-in's own
/// exits 2 with nothing on standard output.
#[must_use]
pub fn stand_in(args: &[String]) -> ExitCode {
    match guard(args) {
        Ok(code) => ExitCode::from(code),
        Err(reason) => {
            eprintln!("{SUBCOMMAND}: {reason}");
            ExitCode::from(FAILED)
        }
    }
}

/// The stand-in's work, returning the exit code to pass through.
fn guard(args: &[String]) -> Result<u8, String> {
    let (shellcheck, rest) = args.split_first().ok_or("no ShellCheck path was given")?;
    let mut script = Vec::new();
    io::stdin()
        .read_to_end(&mut script)
        .map_err(|err| format!("reading the script: {err}"))?;
    let refused = refusals(
        std::str::from_utf8(&script).map_err(|err| format!("the script is not UTF-8: {err}"))?,
    );
    if !refused.is_empty() {
        let json = serde_json::to_string(&refused)
            .map_err(|err| format!("writing the refusals: {err}"))?;
        io::stdout()
            .write_all(json.as_bytes())
            .map_err(|err| format!("writing the refusals: {err}"))?;
        return Ok(1);
    }
    let mut child = spawn::command(Path::new(shellcheck))?
        .args(rest)
        .env_remove("SHELLCHECK_OPTS")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("{shellcheck} could not start: {err}"))?;
    let mut input = child
        .stdin
        .take()
        .ok_or("ShellCheck's standard input was not opened")?;
    // ShellCheck's output is read while the script is written, so neither
    // side waits on a full pipe.
    let writer = std::thread::spawn(move || input.write_all(&script));
    let output = child
        .wait_with_output()
        .map_err(|err| format!("waiting for {shellcheck}: {err}"))?;
    let written = writer
        .join()
        .map_err(|_| "the thread writing the script panicked".to_string())?;
    // actionlint reads any output with a non-zero exit as findings, so nothing
    // reaches standard output until the whole script went in and ShellCheck
    // gave an exit code.
    let code = written
        .map_err(|err| format!("{shellcheck} did not read the whole script: {err}"))
        .and_then(|()| {
            output
                .status
                .code()
                .and_then(|code| u8::try_from(code).ok())
                .ok_or_else(|| format!("{shellcheck} ended without an exit code"))
        });
    let code = match code {
        Ok(code) => code,
        Err(reason) => {
            let _ = io::stderr().write_all(&output.stderr);
            return Err(reason);
        }
    };
    io::stdout()
        .write_all(&output.stdout)
        .and_then(|()| io::stderr().write_all(&output.stderr))
        .map_err(|err| format!("passing ShellCheck's output through: {err}"))?;
    Ok(code)
}

/// A Bun script that reads the tracked workflows named on standard input,
/// separated by NUL, and prints one JSON entry per path: every `shell:` a step
/// or a `defaults.run` carries, with where, or why the file did not parse.
///
/// Keys match in any case, and Bun's YAML parser decodes every escape, so the
/// shell read is the one GitHub reads. A file that holds anything but one
/// mapping, a second document included, reports an error.
pub const WORKFLOW_SHELLS: &str = r"
const paths = (await Bun.stdin.text()).split('\0').filter((path) => path.length > 0);
const mapping = (node) => node !== null && typeof node === 'object' && !Array.isArray(node);
const named = (node, name) =>
  mapping(node) ? Object.keys(node).filter((key) => key.toLowerCase() === name).map((key) => [key, node[key]]) : [];
const report = [];
for (const path of paths) {
  let document;
  try {
    document = Bun.YAML.parse(await Bun.file(path).text());
  } catch (error) {
    report.push({ path, error: String(error) });
    continue;
  }
  if (!mapping(document)) {
    report.push({ path, error: 'the file does not hold one mapping' });
    continue;
  }
  const shells = [];
  const defaults = (node, at) => {
    for (const [d, value] of named(node, 'defaults'))
      for (const [r, run] of named(value, 'run'))
        for (const [s, shell] of named(run, 'shell')) shells.push({ at: `${at}${d}.${r}.${s}`, shell });
  };
  defaults(document, '');
  for (const [j, jobs] of named(document, 'jobs')) {
    for (const [id, job] of mapping(jobs) ? Object.entries(jobs) : []) {
      defaults(job, `${j}.${id}.`);
      for (const [s, steps] of named(job, 'steps')) {
        (Array.isArray(steps) ? steps : []).forEach((step, index) => {
          for (const [k, shell] of named(step, 'shell')) shells.push({ at: `${j}.${id}.${s}[${index}].${k}`, shell });
        });
      }
    }
  }
  report.push({ path, shells });
}
process.stdout.write(JSON.stringify(report));
";

/// One entry of the [`WORKFLOW_SHELLS`] report.
#[derive(serde::Deserialize)]
struct Entry {
    path: String,
    #[serde(default)]
    shells: Vec<Shell>,
    error: Option<String>,
}

/// One `shell:` the report names.
#[derive(serde::Deserialize)]
struct Shell {
    at: String,
    shell: serde_json::Value,
}

/// The paths the [`WORKFLOW_SHELLS`] `report` read, and one sentence for each
/// file it could not read and each shell outside [`SHELLS`].
///
/// # Errors
///
/// Returns the sentence a result row carries when the report is not the JSON
/// the script prints.
pub fn shell_findings(report: &str) -> Result<(Vec<String>, Vec<String>), String> {
    let entries: Vec<Entry> = serde_json::from_str(report)
        .map_err(|err| format!("the workflow shell report does not parse: {err}"))?;
    let mut read = Vec::new();
    let mut found = Vec::new();
    for entry in entries {
        if let Some(error) = entry.error {
            found.push(format!(
                "{:?} does not parse as YAML Bun reads, so the shells its steps run are unknown: {error}",
                entry.path
            ));
        }
        for shell in &entry.shells {
            if !shell
                .shell
                .as_str()
                .is_some_and(|value| SHELLS.contains(&value))
            {
                found.push(format!(
                    "{:?} runs {} under the shell {}, and actionlint hands only a bash or sh script to ShellCheck. Use {}",
                    entry.path,
                    shell.at,
                    shell.shell,
                    SHELLS.join(", ")
                ));
            }
        }
        read.push(entry.path);
    }
    Ok((read, found))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{REFUSAL, SUBCOMMAND, directive, flag_value, refusals, shell_findings};

    /// A directive is refused in every spelling `ShellCheck` honors, and a line
    /// that only mentions the word is not.
    #[test]
    fn a_directive_is_found_in_every_spelling_shellcheck_reads() {
        for (line, wanted) in [
            ("# shellcheck disable=SC2086", true),
            ("echo $a # shellcheck disable=SC2086", true),
            ("#shellcheck disable=SC2086", true),
            ("#\tShellCheck\tdisable=SC2086", true),
            ("  #   SHELLCHECK source=/dev/null disable=SC2086", true),
            ("# shellcheck source='x'disable=SC2086", true),
            ("#\u{200B}shellcheck\u{200B}disable=SC2086", true),
            ("#\u{00A0}shellcheck\u{2009}disable=SC2086", true),
            ("# not this # shellcheck enable=all", true),
            ("# shellcheck", false),
            ("# shellchecked disable=SC2086", false),
            ("# shell check disable=SC2086", false),
            ("echo shellcheck disable=SC2086", false),
            ("# a comment about shellcheck", false),
            ("", false),
        ] {
            assert_eq!(directive(line), wanted, "{line:?}");
        }
    }

    /// Each refused line comes back as one finding in `ShellCheck`'s JSON
    /// shape, numbered as `ShellCheck` numbers it.
    #[test]
    fn a_refusal_takes_shellchecks_finding_shape() {
        let found = refusals("set -eo pipefail\necho ok\n# shellcheck disable=SC2086\necho $a\n");
        assert_eq!(found.len(), 1, "{found:?}");
        let finding = &found[0];
        assert_eq!(finding["line"], 3);
        assert_eq!(finding["endLine"], 3);
        assert_eq!(finding["column"], 1);
        assert_eq!(finding["level"], "error");
        assert_eq!(finding["code"], 0);
        assert_eq!(finding["file"], "-");
        let message = finding["message"].as_str().expect("a message");
        assert!(message.starts_with(REFUSAL), "{message}");
        assert!(message.contains("# shellcheck disable=SC2086"), "{message}");
        assert!(refusals("set -e\necho \"$a\"\n").is_empty());
    }

    /// The flag value names both programs single-quoted with forward slashes,
    /// and a path holding a single quote is refused.
    #[test]
    fn the_flag_value_quotes_both_paths() {
        assert_eq!(
            flag_value(
                Path::new(r"C:\a b\xtask.exe"),
                Path::new(r"C:\mise\shellcheck.exe")
            ),
            Ok(format!(
                "'C:/a b/xtask.exe' {SUBCOMMAND} 'C:/mise/shellcheck.exe'"
            ))
        );
        let refused = flag_value(Path::new("/it's/xtask"), Path::new("/bin/shellcheck"))
            .expect_err("a single quote");
        assert!(refused.contains("single quote"), "{refused}");
    }

    /// A shell outside bash, sh and pwsh is refused wherever a step or a
    /// default sets it, and so is a file Bun could not read.
    #[test]
    fn a_shell_outside_the_list_is_refused() {
        let report = r#"[
            {"path": ".github/workflows/a.yml", "shells": [
                {"at": "defaults.run.shell", "shell": "bash"},
                {"at": "jobs.b.steps[0].shell", "shell": "pwsh"},
                {"at": "jobs.b.defaults.run.shell", "shell": "sh"}
            ]},
            {"path": ".github/workflows/b.yml", "shells": [
                {"at": "jobs.c.steps[1].shell", "shell": "/bin/bash -e {0}"},
                {"at": "jobs.c.steps[2].SHELL", "shell": 3}
            ]},
            {"path": ".github/workflows/c.yml", "error": "SyntaxError: bad"}
        ]"#;
        let (read, found) = shell_findings(report).expect("a report");
        assert_eq!(
            read,
            [
                ".github/workflows/a.yml",
                ".github/workflows/b.yml",
                ".github/workflows/c.yml"
            ]
        );
        assert_eq!(
            found,
            [
                "\".github/workflows/b.yml\" runs jobs.c.steps[1].shell under the shell \"/bin/bash -e {0}\", and actionlint hands only a bash or sh script to ShellCheck. Use bash, sh, pwsh",
                "\".github/workflows/b.yml\" runs jobs.c.steps[2].SHELL under the shell 3, and actionlint hands only a bash or sh script to ShellCheck. Use bash, sh, pwsh",
                "\".github/workflows/c.yml\" does not parse as YAML Bun reads, so the shells its steps run are unknown: SyntaxError: bad",
            ]
        );
        let broken = shell_findings("not json").expect_err("a broken report");
        assert!(
            broken.starts_with("the workflow shell report does not parse"),
            "{broken}"
        );
    }
}
