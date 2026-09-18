//! Tests over the repository's own configuration.
//!
//! Each test reads a committed file and asserts a property the workspace
//! depends on: every entry a manifest declares is reachable, every document
//! restates what the code declares, and every tool the gate installs is pinned
//! in one place. They live here because the files they read have no other test.

use std::path::{Path, PathBuf};

use crate::workspace_root;

/// Every `.rs` file under `dir`, recursively.
fn rust_sources(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(rust_sources(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
    found
}

/// Every name a member manifest takes from the workspace table.
///
/// A member opts in with `<name>.workspace = true`, in its own dependency
/// tables and in any `[target.'cfg(..)'.dependencies]` table.
fn names_taken_from_the_workspace(manifest: &toml::Value) -> Vec<String> {
    const TABLES: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];
    let mut out: Vec<String> = Vec::new();
    let mut collect = |table: Option<&toml::Value>| {
        let Some(table) = table.and_then(toml::Value::as_table) else {
            return;
        };
        for (name, entry) in table {
            let takes_it = entry
                .get("workspace")
                .and_then(toml::Value::as_bool)
                .unwrap_or(false);
            if takes_it {
                out.push(name.clone());
            }
        }
    };
    for table in TABLES {
        collect(manifest.get(table));
    }
    if let Some(targets) = manifest.get("target").and_then(toml::Value::as_table) {
        for spec in targets.values() {
            for table in TABLES {
                collect(spec.get(table));
            }
        }
    }
    out
}

/// Every string literal in a JavaScript source, paired with the bracket depth it
/// sits at, in source order.
///
/// Characters inside a literal open no bracket and comments are skipped, so a
/// URL, an apostrophe in a sentence and a commented-out list all read correctly.
fn js_string_literals(source: &str) -> Vec<(usize, String)> {
    let chars: Vec<char> = source.chars().collect();
    let mut found: Vec<(usize, String)> = Vec::new();
    let mut depth: usize = 0;
    let mut index = 0;
    while index < chars.len() {
        match chars[index] {
            '/' if chars.get(index + 1) == Some(&'/') => {
                while index < chars.len() && chars[index] != '\n' {
                    index += 1;
                }
            }
            '/' if chars.get(index + 1) == Some(&'*') => {
                index += 2;
                while index < chars.len()
                    && !(chars[index] == '*' && chars.get(index + 1) == Some(&'/'))
                {
                    index += 1;
                }
                index += 2;
            }
            '[' => {
                depth += 1;
                index += 1;
            }
            ']' => {
                depth = depth.saturating_sub(1);
                index += 1;
            }
            quote @ ('\'' | '"' | '`') => {
                index += 1;
                let mut text = String::new();
                while index < chars.len() && chars[index] != quote {
                    if chars[index] == '\\' {
                        index += 1;
                        if index >= chars.len() {
                            break;
                        }
                    }
                    text.push(chars[index]);
                    index += 1;
                }
                index += 1;
                found.push((depth, text));
            }
            _ => index += 1,
        }
    }
    found
}

/// The scope list `commitlint.config.js` enforces.
///
/// The `scope-enum` rule is `[level, applicability, [scopes]]`, so the scopes
/// are the run of literals two brackets deeper than the rule's own name. An
/// empty result means the file declares no such rule.
fn commitlint_scopes(source: &str) -> Vec<String> {
    let literals = js_string_literals(source);
    let Some(rule) = literals.iter().position(|(_, text)| text == "scope-enum") else {
        return Vec::new();
    };
    let wanted = literals[rule].0 + 2;
    literals[rule + 1..]
        .iter()
        .skip_while(|(depth, _)| *depth != wanted)
        .take_while(|(depth, _)| *depth == wanted)
        .map(|(_, text)| text.clone())
        .collect()
}

/// One `##` section of a markdown document, its heading excluded.
///
/// A heading is a stabler anchor than the sentence under it, so a section can be
/// reworded without moving what reads it.
fn section<'a>(markdown: &'a str, heading: &str) -> Option<&'a str> {
    let opening = format!("\n## {heading}\n");
    let start = markdown.find(&opening)? + opening.len();
    let rest = &markdown[start..];
    Some(rest.find("\n## ").map_or(rest, |end| &rest[..end]))
}

/// Every inline code span in a markdown fragment.
fn code_spans(text: &str) -> Vec<String> {
    text.split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

/// Every paragraph of a markdown fragment that is a bare list of inline code
/// spans, as the spans it holds.
///
/// Anchoring on the paragraph's shape rather than on the sentence above it means
/// rewording the section around it leaves the check working.
fn code_span_lists(markdown: &str) -> Vec<Vec<String>> {
    markdown
        .split("\n\n")
        .filter_map(|paragraph| {
            let spans = code_spans(paragraph);
            if spans.is_empty() {
                return None;
            }
            let mut rest = paragraph.to_string();
            for span in &spans {
                rest = rest.replacen(&format!("`{span}`"), "", 1);
            }
            rest.chars()
                .all(|c| c == ',' || c == '.' || c.is_whitespace())
                .then_some(spans)
        })
        .collect()
}

/// The first cell of every body row of the first markdown table in a fragment,
/// with its inline code markers removed.
fn table_first_column(markdown: &str) -> Vec<String> {
    markdown
        .lines()
        .filter(|line| line.starts_with('|'))
        .skip(2)
        .filter_map(|line| {
            let cell = line.split('|').nth(1)?.trim();
            Some(cell.trim_matches('`').to_string())
        })
        .collect()
}

/// The package an install command names, for a command that is a
/// `cargo install`.
///
/// The flag and the package can come in either order, so the package is the
/// first word past `cargo install` that is not a flag.
fn cargo_install_package(install: &str) -> Option<&str> {
    let mut words = install.split_whitespace();
    if words.next()? != "cargo" || words.next()? != "install" {
        return None;
    }
    words.find(|word| !word.starts_with('-'))
}

#[cfg(test)]
mod tests {
    use super::{
        cargo_install_package, code_span_lists, commitlint_scopes, names_taken_from_the_workspace,
        rust_sources, section, table_first_column, workspace_root,
    };
    use crate::check::{PREFERRED_TEST_RUNNER, STEPS};

    /// A file at the workspace root, read whole.
    fn read(relative: &str) -> String {
        let path = workspace_root().join(relative);
        std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
    }

    /// The pinned tools, as `(name, version)`.
    fn pinned_tools() -> Vec<(String, String)> {
        read(".github/cargo-tools")
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(|line| {
                let (name, version) = line
                    .split_once('@')
                    .unwrap_or_else(|| panic!("{line:?} is not name@version"));
                assert!(!version.is_empty(), "{line:?} has no version");
                (name.to_string(), version.to_string())
            })
            .collect()
    }

    /// A crate's manifest under the workspace, with the names its cargo-machete
    /// ignore list carries.
    fn ignore_lists() -> Vec<(std::path::PathBuf, Vec<String>)> {
        let root = workspace_root();
        let mut manifests: Vec<std::path::PathBuf> = vec![root.join("xtask").join("Cargo.toml")];
        if let Ok(entries) = std::fs::read_dir(root.join("crates")) {
            manifests.extend(
                entries
                    .filter_map(std::result::Result::ok)
                    .map(|entry| entry.path().join("Cargo.toml"))
                    .filter(|path| path.is_file()),
            );
        }
        manifests
            .into_iter()
            .filter_map(|manifest| {
                let text = std::fs::read_to_string(&manifest).ok()?;
                let value: toml::Value = toml::from_str(&text).ok()?;
                let ignored = value
                    .get("package")?
                    .get("metadata")?
                    .get("cargo-machete")?
                    .get("ignored")?
                    .as_array()?
                    .iter()
                    .filter_map(|name| name.as_str().map(str::to_string))
                    .collect();
                Some((manifest, ignored))
            })
            .collect()
    }

    /// The workspace manifest and every member manifest it lists.
    fn workspace_and_members() -> (toml::Value, Vec<(std::path::PathBuf, toml::Value)>) {
        let root = workspace_root();
        let text = std::fs::read_to_string(root.join("Cargo.toml"))
            .expect("the workspace manifest is readable");
        let workspace: toml::Value = toml::from_str(&text).expect("the workspace manifest parses");
        let members: Vec<String> = workspace
            .get("workspace")
            .and_then(|w| w.get("members"))
            .and_then(toml::Value::as_array)
            .expect("the workspace lists members")
            .iter()
            .filter_map(|m| m.as_str().map(str::to_string))
            .collect();
        let loaded = members
            .into_iter()
            .map(|member| {
                let path = root.join(&member).join("Cargo.toml");
                let text = std::fs::read_to_string(&path)
                    .unwrap_or_else(|err| panic!("reading {}: {err}", path.display()));
                let value = toml::from_str(&text)
                    .unwrap_or_else(|err| panic!("parsing {}: {err}", path.display()));
                (path, value)
            })
            .collect();
        (workspace, loaded)
    }

    /// Every entry in `[workspace.dependencies]` is taken by some member.
    ///
    /// `cargo machete` walks each crate's own tables against that crate's
    /// sources, so an entry here that no member names is invisible to it. Such
    /// an entry pins a version and carries a name that nothing can reach, which
    /// is a trap for whoever reads the table next and assumes it is in use.
    ///
    /// The five `ember-*` path entries are checked like any other. They are
    /// ordinary entries that happen to point inside the workspace, and a crate
    /// dropped from every member's dependencies would leave one orphaned in
    /// exactly the same way.
    #[test]
    fn every_workspace_dependency_is_taken_by_a_member() {
        let (workspace, members) = workspace_and_members();
        let declared: Vec<String> = workspace
            .get("workspace")
            .and_then(|w| w.get("dependencies"))
            .and_then(toml::Value::as_table)
            .expect("the workspace declares dependencies")
            .keys()
            .cloned()
            .collect();
        assert!(!declared.is_empty(), "the workspace table is not empty");

        let mut taken: Vec<String> = Vec::new();
        for (_, manifest) in &members {
            taken.extend(names_taken_from_the_workspace(manifest));
        }
        let orphaned: Vec<&String> = declared
            .iter()
            .filter(|name| !taken.contains(name))
            .collect();
        assert!(
            orphaned.is_empty(),
            "[workspace.dependencies] entries no member takes, so nothing can reach them: {}",
            orphaned
                .iter()
                .map(|name| name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    /// Every member inherits the workspace lint table.
    ///
    /// `[workspace.lints]` is opt in per crate. A member with no `[lints]`
    /// block compiles with none of `missing_docs`, `clippy::pedantic`,
    /// `undocumented_unsafe_blocks` or `unsafe_op_in_unsafe_fn`, and the gate
    /// stays green while saying nothing about it. The crates here write inline
    /// detours, so a missing block is a crate writing unsafe code with the
    /// unsafe lints switched off.
    #[test]
    fn every_member_inherits_the_workspace_lints() {
        let (_, members) = workspace_and_members();
        assert!(!members.is_empty(), "the workspace lists members");
        let missing: Vec<String> = members
            .iter()
            .filter(|(_, manifest)| {
                !manifest
                    .get("lints")
                    .and_then(|lints| lints.get("workspace"))
                    .and_then(toml::Value::as_bool)
                    .unwrap_or(false)
            })
            .map(|(path, _)| path.display().to_string())
            .collect();
        assert!(
            missing.is_empty(),
            "these members do not carry `[lints] workspace = true`, so the workspace lints are off for them: {}",
            missing.join(", ")
        );
    }

    /// Every key in `[workspace.package]` is taken by some member.
    ///
    /// The same blind spot in a table that costs less when it drifts: an unused
    /// key here pins no version and reaches no registry, it just describes a
    /// crate nothing inherits.
    #[test]
    fn every_workspace_package_key_is_taken_by_a_member() {
        let (workspace, members) = workspace_and_members();
        let declared: Vec<String> = workspace
            .get("workspace")
            .and_then(|w| w.get("package"))
            .and_then(toml::Value::as_table)
            .expect("the workspace declares package keys")
            .keys()
            .cloned()
            .collect();

        let mut taken: Vec<String> = Vec::new();
        for (_, manifest) in &members {
            let Some(package) = manifest.get("package").and_then(toml::Value::as_table) else {
                continue;
            };
            for (key, entry) in package {
                let inherited = entry
                    .get("workspace")
                    .and_then(toml::Value::as_bool)
                    .unwrap_or(false);
                if inherited {
                    taken.push(key.clone());
                }
            }
        }
        let orphaned: Vec<&String> = declared.iter().filter(|key| !taken.contains(key)).collect();
        assert!(
            orphaned.is_empty(),
            "[workspace.package] keys no member inherits: {}",
            orphaned
                .iter()
                .map(|key| key.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    /// An ignored dependency is one no module references. The moment code
    /// starts using it, its name has to leave the ignore list, or machete
    /// would stop watching a dependency that could later fall out of use
    /// again. This is what makes the lists self-pruning.
    #[test]
    fn every_ignored_dependency_is_still_unreferenced() {
        let lists = ignore_lists();
        assert!(
            !lists.is_empty(),
            "the seven skeleton manifests carry ignore lists"
        );
        let mut stale: Vec<String> = Vec::new();
        for (manifest, ignored) in lists {
            let crate_dir = manifest.parent().expect("a manifest has a directory");
            let files = rust_sources(&crate_dir.join("src"));
            for name in ignored {
                let ident = name.replace('-', "_");
                let needles = [
                    format!("use {ident}"),
                    format!("{ident}::"),
                    format!("extern crate {ident}"),
                ];
                for file in &files {
                    let text = std::fs::read_to_string(file).unwrap_or_default();
                    if needles.iter().any(|needle| text.contains(needle.as_str())) {
                        stale.push(format!(
                            "{} ignores {name}, but {} uses it",
                            manifest.display(),
                            file.display()
                        ));
                        break;
                    }
                }
            }
        }
        assert!(
            stale.is_empty(),
            "prune these names from their ignore lists: {}",
            stale.join("; ")
        );
    }

    /// The commit scope vocabulary lives in three places: the constant
    /// `cargo xtask scopes` prints, the rule the commit hook enforces, and the
    /// sentence `CONTRIBUTING.md` restates. A contributor who reads one and a
    /// hook that enforces another disagree silently.
    #[test]
    fn every_copy_of_the_scope_list_agrees() {
        let declared: Vec<String> = crate::SCOPES
            .iter()
            .map(|scope| (*scope).to_string())
            .collect();
        assert!(!declared.is_empty(), "xtask/src/main.rs declares no scopes");

        let commitlint = commitlint_scopes(&read("commitlint.config.js"));
        let contributing = read("CONTRIBUTING.md");
        let commits = section(&contributing, "Commit messages")
            .expect("CONTRIBUTING.md has a Commit messages section");
        let documented = code_span_lists(commits);
        assert_eq!(
            documented.len(),
            1,
            "the Commit messages section holds {} paragraphs that are a bare list of code spans, so which one restates the scopes is ambiguous",
            documented.len()
        );

        let mut disagree: Vec<String> = Vec::new();
        if commitlint != declared {
            disagree.push(format!(
                "commitlint.config.js scope-enum has {commitlint:?}"
            ));
        }
        if documented[0] != declared {
            disagree.push(format!("CONTRIBUTING.md restates {:?}", documented[0]));
        }
        assert!(
            disagree.is_empty(),
            "xtask/src/main.rs SCOPES has {declared:?}, and {}",
            disagree.join("; ")
        );
    }

    /// The README's crate table is the map a reader opens first. A member it
    /// omits is a crate nobody looking at the table knows exists.
    ///
    /// The order is asserted too, because the table and `[workspace.members]`
    /// are both in dependency order and are meant to be read side by side.
    #[test]
    fn the_crate_table_lists_every_workspace_member_in_order() {
        let workspace: toml::Value =
            toml::from_str(&read("Cargo.toml")).expect("the workspace manifest parses");
        let members: Vec<String> = workspace
            .get("workspace")
            .and_then(|w| w.get("members"))
            .and_then(toml::Value::as_array)
            .expect("the workspace lists members")
            .iter()
            .filter_map(|member| member.as_str())
            .map(|member| member.rsplit('/').next().unwrap_or(member).to_string())
            .collect();

        let readme = read("README.md");
        let crates = section(&readme, "Crates").expect("README.md has a Crates section");
        let tabled = table_first_column(crates);

        assert_eq!(
            tabled, members,
            "the README crate table and [workspace.members] disagree"
        );
    }

    /// Every tool the gate installs with `cargo install` carries a version, in
    /// one file. A second copy of a version is a copy that drifts.
    ///
    /// The check runs both ways. A step whose tool is unpinned installs
    /// whatever the registry serves today, and a pinned entry no step installs
    /// is a version continuous integration fetches for nothing.
    #[test]
    fn the_pinned_tool_file_and_the_gate_name_the_same_tools() {
        let pinned = pinned_tools();
        assert!(!pinned.is_empty(), "the pinned tool file names nothing");

        // The tests step prefers cargo-nextest at run time rather than
        // declaring it, so its name reaches the pinned file from here.
        assert_eq!(
            pinned
                .iter()
                .filter(|(name, _)| name == PREFERRED_TEST_RUNNER)
                .count(),
            1,
            "{PREFERRED_TEST_RUNNER} is not pinned once in .github/cargo-tools"
        );
        let mut installed: Vec<String> = vec![PREFERRED_TEST_RUNNER.to_string()];
        for step in &STEPS {
            let Some(package) = cargo_install_package(step.install) else {
                continue;
            };
            let found = pinned.iter().filter(|(name, _)| name == package).count();
            assert_eq!(
                found, 1,
                "{package} appears {found} times in .github/cargo-tools"
            );
            installed.push(package.to_string());
        }

        let unused: Vec<&String> = pinned
            .iter()
            .map(|(name, _)| name)
            .filter(|name| !installed.contains(name))
            .collect();
        assert!(
            unused.is_empty(),
            "no gate step installs {unused:?} from .github/cargo-tools"
        );
    }

    /// Ember's manifests are formatted by `taplo`, which reads `.taplo.toml`.
    /// A gate step whose configuration is absent formats whatever the tool
    /// defaults to.
    #[test]
    fn the_toml_formatter_has_a_configuration_file() {
        let config: toml::Value = toml::from_str(&read(".taplo.toml")).expect(".taplo.toml parses");

        assert!(
            config.get("include").is_some(),
            ".taplo.toml names no files to format"
        );
    }

    /// The parser reads the rule's nested array and leaves every other literal
    /// in the file alone, including one inside a comment and one holding a
    /// bracket.
    #[test]
    fn commitlint_scopes_reads_the_nested_rule_array() {
        let cases = [
            (
                "\
export default {
  extends: ['@commitlint/config-conventional'],
  ignores: [(message) => message.includes('Signed-off-by: dependabot[bot]')],
  rules: {
    'scope-enum': [2, 'always', ['one', 'two']],
    'body-max-line-length': [2, 'always', 72],
  },
};
",
                vec!["one", "two"],
            ),
            (
                "\
export default {
  rules: {
    'scope-enum': [
      2,
      'always',
      // A list at https://example.invalid that isn't the rule's own.
      /* 'commented' */
      ['one', 'two', 'three'],
    ],
  },
};
",
                vec!["one", "two", "three"],
            ),
            ("export default { rules: {} };", vec![]),
        ];

        for (source, expected) in cases {
            assert_eq!(commitlint_scopes(source), expected, "{source}");
        }
    }

    /// A paragraph of prose holding code spans is not a list, a bullet is not a
    /// bare paragraph, and a table yields its first column alone.
    #[test]
    fn the_markdown_readers_take_the_shapes_they_name() {
        let markdown = "\
# Title

## First

Run `cargo xtask scopes` for the live list.

`one`, `two`, `three`.

- `four`, `five`

## Second

| Crate   | Holds        |
| ------- | ------------ |
| `alpha` | The first    |
| `beta`  | The second   |

## Third

three
";
        let first = section(markdown, "First").expect("a First section");
        assert_eq!(code_span_lists(first), [["one", "two", "three"]]);

        let second = section(markdown, "Second").expect("a Second section");
        assert_eq!(table_first_column(second), ["alpha", "beta"]);

        assert_eq!(section(markdown, "Fourth"), None);
    }

    /// The flag and the package name come in either order, and a step that
    /// installs through rustup or bun names no package at all.
    #[test]
    fn cargo_install_package_reads_either_argument_order() {
        let cases = [
            ("cargo install --locked cargo-deny", Some("cargo-deny")),
            ("cargo install cargo-deny --locked", Some("cargo-deny")),
            ("cargo install --locked taplo-cli", Some("taplo-cli")),
            ("rustup component add rustfmt", None),
            ("rustup toolchain install", None),
            ("install Bun from https://bun.sh", None),
            ("cargo install", None),
            ("cargo install --locked", None),
            ("", None),
        ];

        for (install, expected) in cases {
            assert_eq!(cargo_install_package(install), expected, "{install:?}");
        }
    }
}
