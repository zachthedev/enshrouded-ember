//! Tests over the repository's own configuration.
//!
//! Each test reads a committed file and asserts a property the workspace
//! depends on: every entry a manifest declares is reachable, every document
//! restates what the code declares, and every tool the gate installs is pinned
//! in one place. They live here because the files they read have no other test.

use std::path::{Path, PathBuf};

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

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

/// Whether `text` references the crate `ident`.
///
/// A reference is the name opening a path, or following `use` or
/// `extern crate`. The name has to end at a non-identifier character, because
/// `cargo machete` reads `serde` and `serde_json` as two crates and a check
/// that reads the first inside the second demands a prune that makes machete
/// fail on the same name.
fn references_crate(text: &str, ident: &str) -> bool {
    text.match_indices(ident).any(|(at, _)| {
        let rest: &str = &text[at + ident.len()..];
        let head: &str = &text[..at];
        let whole_word = head
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric() && c != '_')
            && rest
                .chars()
                .next()
                .is_none_or(|c| !c.is_alphanumeric() && c != '_');

        whole_word
            && (rest.starts_with("::") || head.ends_with("use ") || head.ends_with("extern crate "))
    })
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

/// The events of one `##` section of a Markdown document, from the block after
/// its heading to the block before the next heading at the same level or above.
/// `None` when the document has no such heading.
///
/// A heading is a stabler anchor than the sentence under it, so a section can be
/// reworded without moving what reads it.
fn section<'a>(markdown: &'a str, heading: &str) -> Option<Vec<Event<'a>>> {
    let events: Vec<Event<'a>> = Parser::new_ext(markdown, Options::ENABLE_TABLES).collect();
    let mut index = 0;
    while index < events.len() {
        if let Event::Start(Tag::Heading {
            level: HeadingLevel::H2,
            ..
        }) = &events[index]
        {
            let close = index
                + events[index..]
                    .iter()
                    .position(|event| matches!(event, Event::End(TagEnd::Heading(_))))?;
            let title: String = events[index + 1..close]
                .iter()
                .filter_map(text_of)
                .collect();
            if title == heading {
                let body = &events[close + 1..];
                let end = body
                    .iter()
                    .position(|event| {
                        matches!(event, Event::Start(Tag::Heading { level, .. }) if *level <= HeadingLevel::H2)
                    })
                    .unwrap_or(body.len());
                return Some(body[..end].to_vec());
            }
            index = close;
        }
        index += 1;
    }
    None
}

/// The text an inline event carries, code spans included.
fn text_of<'e>(event: &'e Event<'_>) -> Option<&'e str> {
    match event {
        Event::Text(text) | Event::Code(text) => Some(&**text),
        _ => None,
    }
}

/// Every paragraph in `events` that is a bare list of inline code spans, as the
/// spans it holds.
///
/// Anchoring on the paragraph's shape rather than on the sentence above it means
/// rewording the section around it leaves the check working. A bullet is not a
/// paragraph, so a bulleted list of code spans is not read.
fn code_span_lists(events: &[Event<'_>]) -> Vec<Vec<String>> {
    let mut lists: Vec<Vec<String>> = Vec::new();
    let mut spans: Vec<String> = Vec::new();
    let mut bare = true;
    let mut inside = false;
    for event in events {
        match event {
            Event::Start(Tag::Paragraph) => {
                inside = true;
                bare = true;
                spans.clear();
            }
            Event::End(TagEnd::Paragraph) => {
                inside = false;
                if bare && !spans.is_empty() {
                    lists.push(std::mem::take(&mut spans));
                }
            }
            Event::Code(code) if inside => spans.push(code.to_string()),
            Event::Text(text) if inside => {
                bare &= text
                    .chars()
                    .all(|c| c == ',' || c == '.' || c.is_whitespace());
            }
            Event::SoftBreak | Event::HardBreak => {}
            _ if inside => bare = false,
            _ => {}
        }
    }
    lists
}

/// The first cell of every body row of every table in `events`, as text with the
/// code markers dropped.
fn first_column(events: &[Event<'_>]) -> Vec<String> {
    let mut column: Vec<String> = Vec::new();
    let mut cell = String::new();
    let mut first_cell_of_row = false;
    let mut in_head = false;
    for event in events {
        match event {
            Event::Start(Tag::TableHead) => in_head = true,
            Event::End(TagEnd::TableHead) => in_head = false,
            Event::Start(Tag::TableRow) => first_cell_of_row = true,
            Event::Start(Tag::TableCell) => cell.clear(),
            Event::End(TagEnd::TableCell) => {
                if first_cell_of_row && !in_head {
                    column.push(std::mem::take(&mut cell));
                }
                first_cell_of_row = false;
            }
            Event::Text(text) | Event::Code(text) => cell.push_str(text),
            _ => {}
        }
    }
    column
}

/// Number words the prose check reads as a count.
///
/// "One" is left out, because a sentence naming one step names a step rather
/// than counting the gate's. Ordinals are left out, because "the first step" is
/// a position.
const COUNT_WORDS: &[&str] = &[
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
    "twenty",
    "thirty",
    "forty",
    "fifty",
    "sixty",
    "seventy",
    "eighty",
    "ninety",
    "dozen",
];

/// The tens a hyphenated count opens with, as in "twenty-one".
const TENS_WORDS: &[&str] = &[
    "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];

/// The units a hyphenated count closes with, as in "twenty-one".
const UNIT_WORDS: &[&str] = &[
    "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
];

/// The nouns whose count the prose check refuses when the count comes first:
/// the gate's steps under each name prose gives them, and the pinned tools.
const COUNTED_NOUNS: &[&str] = &[
    "step", "steps", "stage", "stages", "check", "checks", "command", "commands", "tool", "tools",
];

/// The nouns the check also reads with the count after them, as in
/// `Steps: 13`. "Checks" and "commands" are left out, because they are verbs
/// as often as nouns, as in `the gate checks two things`.
const COUNTED_BEFORE: &[&str] = &["steps", "stages", "tools"];

/// Whether `word` is a count: a run of up to three digits, a number word from
/// two up, or a hyphenated tens and unit. A longer run of digits is an id or a
/// measurement, never a count of steps or tools.
fn is_count(word: &str) -> bool {
    if (1..=3).contains(&word.len()) && word.bytes().all(|b| b.is_ascii_digit()) {
        return true;
    }
    if COUNT_WORDS.contains(&word) {
        return true;
    }
    word.split_once('-')
        .is_some_and(|(tens, unit)| TENS_WORDS.contains(&tens) && UNIT_WORDS.contains(&unit))
}

/// The clauses of `prose`, each as its lowercase words.
///
/// An inline code span reads as the one word `code`, so a command name between
/// a count and its noun is not three words of distance, and a count quoted as
/// code is literal text rather than a claim. A run of three or more backticks
/// is a fence rather than a span, so the text inside a fenced block is read. A
/// sentence ends at `.`, `!` or `?` followed by whitespace, and a clause at
/// `;`, `,` and brackets. A dot inside a word, as in `crates.io`, ends nothing.
/// Colons and table pipes join, so a count after a colon or in the next table
/// cell reads in the same clause. Apostrophes stay inside a word, so `gate's`
/// is one word.
fn clauses(prose: &str) -> Vec<Vec<String>> {
    let chars: Vec<char> = prose.chars().collect();
    let mut plain = String::with_capacity(prose.len());
    let mut in_code = false;
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '`' {
            let run = chars[index..].iter().take_while(|c| **c == '`').count();
            if run < 3 {
                if !in_code {
                    plain.push_str(" code ");
                }
                in_code = !in_code;
            } else {
                plain.push(' ');
            }
            index += run;
            continue;
        }
        if !in_code {
            plain.push(chars[index]);
        }
        index += 1;
    }
    let plain = paths_as_words(&plain);

    let chars: Vec<char> = plain.chars().collect();
    let mut clauses: Vec<Vec<String>> = Vec::new();
    let mut current = String::new();
    for (index, c) in chars.iter().enumerate() {
        let ends_sentence = matches!(c, '.' | '!' | '?')
            && chars.get(index + 1).is_none_or(|next| next.is_whitespace());
        if ends_sentence || ";,()[]".contains(*c) {
            clauses.push(words(&current));
            current.clear();
        } else {
            current.push(*c);
        }
    }
    clauses.push(words(&current));
    clauses
}

/// `text` with every whitespace-separated token that holds a `/` read as the
/// one word `path`, so a file path or a URL between two words is not several
/// words, and a directory name in it is not a noun. Punctuation closing the
/// token stays, so a sentence ending in a path still ends.
fn paths_as_words(text: &str) -> String {
    text.split_whitespace()
        .map(|token| {
            if !token.contains('/') {
                return token.to_string();
            }
            let closing: String = token
                .chars()
                .rev()
                .take_while(|c| ".,;!?)]".contains(*c))
                .collect::<Vec<char>>()
                .into_iter()
                .rev()
                .collect();
            format!("path{closing}")
        })
        .collect::<Vec<String>>()
        .join(" ")
}

/// The lowercase words of a clause, split at anything that is not a letter, a
/// digit, a hyphen or an apostrophe.
fn words(clause: &str) -> Vec<String> {
    clause
        .split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '\''))
        .map(|word| word.trim_matches(['\'', '-']))
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Every phrase in `prose` that states how many gate steps or pinned tools there
/// are.
///
/// Three shapes count, each inside one clause:
///
/// - a count, then one of [`COUNTED_NOUNS`] with at most two words between
/// - one of [`COUNTED_BEFORE`] with a count straight after it, as in
///   `Steps: 13`, or "step count" or "tool count" then a count with at most two
///   words between. A plural noun takes no gap, because "steps" is also a verb.
/// - a count hyphenated to one of [`COUNTED_NOUNS`], as in `eleven-step`
fn stated_counts(prose: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for words in clauses(prose) {
        for (index, word) in words.iter().enumerate() {
            let following = |from: usize| words.iter().skip(from).take(3);
            if let Some((head, tail)) = word.rsplit_once('-')
                && is_count(head)
                && COUNTED_NOUNS.contains(&tail)
            {
                found.push(word.clone());
            } else if is_count(word)
                && let Some(offset) =
                    following(index + 1).position(|next| COUNTED_NOUNS.contains(&next.as_str()))
            {
                found.push(words[index..=index + 1 + offset].join(" "));
            } else {
                let (noun_end, reach) = if COUNTED_BEFORE.contains(&word.as_str()) {
                    (Some(index), 1)
                } else if matches!(word.as_str(), "step" | "tool")
                    && words.get(index + 1).is_some_and(|next| next == "count")
                {
                    (Some(index + 1), 3)
                } else {
                    (None, 0)
                };
                if let Some(end) = noun_end
                    && let Some(offset) = words
                        .iter()
                        .skip(end + 1)
                        .take(reach)
                        .position(|next| is_count(next))
                {
                    found.push(words[index..=end + 1 + offset].join(" "));
                }
            }
        }
    }
    found
}

/// The text of a `//`, `///`, `//!` or block comment line, markers removed.
fn slash_comment(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    (trimmed.starts_with("//") || trimmed.starts_with('*') || trimmed.starts_with("/*"))
        .then(|| trimmed.trim_start_matches(['/', '*', '!']))
}

/// The text of a `#` comment, whether it opens the line or trails a value.
fn hash_comment(line: &str) -> Option<&str> {
    line.trim_start()
        .strip_prefix('#')
        .or_else(|| line.split_once(" #").map(|(_, rest)| rest))
}

/// The value of a YAML `name:` key, which the Actions page shows as prose.
fn yaml_name(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let trimmed = trimmed.strip_prefix("- ").unwrap_or(trimmed);
    let value = trimmed.strip_prefix("name:")?;
    let value = value.split_once(" #").map_or(value, |(before, _)| before);
    Some(value.trim().trim_matches(['"', '\'']))
}

/// How the prose check reads a file, or `None` when the file carries no prose
/// it reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProseKind {
    /// Markdown and the issue forms, read whole, code blocks included.
    Whole,
    /// Rust, TypeScript and JavaScript, read for their comment lines.
    SlashComments,
    /// YAML, read for its `#` comments and its `name:` values.
    Yaml,
    /// TOML and the extensionless hook, pin and ignore files, read for their
    /// `#` comments.
    HashComments,
}

/// The way the prose check reads `path`.
fn prose_kind(path: &Path) -> Option<ProseKind> {
    let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
    let in_forms = path
        .components()
        .any(|part| part.as_os_str() == "ISSUE_TEMPLATE");
    if extension == "md" || in_forms {
        return Some(ProseKind::Whole);
    }
    match extension {
        "rs" | "ts" | "js" | "mjs" | "cjs" => Some(ProseKind::SlashComments),
        "yml" | "yaml" => Some(ProseKind::Yaml),
        "toml" | "" => Some(ProseKind::HashComments),
        _ => None,
    }
}

/// The prose `text` carries, as paragraphs, given the file it came from.
///
/// Consecutive comment lines join into one paragraph, so a phrase wrapped
/// across two of them reads whole. A YAML `name:` value is a paragraph of its
/// own. A file [`prose_kind`] does not read yields nothing.
fn prose_paragraphs(path: &Path, text: &str) -> Vec<String> {
    let Some(kind) = prose_kind(path) else {
        return Vec::new();
    };
    if kind == ProseKind::Whole {
        return text.split("\n\n").map(str::to_string).collect();
    }
    let comment: fn(&str) -> Option<&str> = match kind {
        ProseKind::SlashComments => slash_comment,
        _ => hash_comment,
    };
    let mut paragraphs: Vec<String> = Vec::new();
    let mut current = String::new();
    for line in text.lines() {
        if kind == ProseKind::Yaml
            && let Some(name) = yaml_name(line)
        {
            if !current.is_empty() {
                paragraphs.push(std::mem::take(&mut current));
            }
            paragraphs.push(name.to_string());
            continue;
        }
        match comment(line).map(str::trim) {
            Some(prose) if !prose.is_empty() => {
                current.push(' ');
                current.push_str(prose);
            }
            _ if !current.is_empty() => paragraphs.push(std::mem::take(&mut current)),
            _ => {}
        }
    }
    if !current.is_empty() {
        paragraphs.push(current);
    }
    paragraphs
}

/// Every file under `dir` the prose check reads, past the directories that hold
/// build output, installed packages, fetched data, vendored checkouts and git's
/// own store.
fn prose_files(dir: &Path) -> Vec<PathBuf> {
    const SKIPPED: &[&str] = &[".git", ".cache", "node_modules", "target", "vendor"];
    let mut found: Vec<PathBuf> = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let skipped = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| SKIPPED.contains(&name));
            if !skipped {
                found.extend(prose_files(&path));
            }
        } else {
            found.push(path);
        }
    }
    found
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
    use std::path::Path;

    use super::{
        cargo_install_package, code_span_lists, first_column, names_taken_from_the_workspace,
        prose_files, prose_kind, prose_paragraphs, references_crate, rust_sources, section,
        stated_counts, workspace_root,
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

    /// The pinned Go tools, each as the whole `module/path@version` entry.
    ///
    /// The path is kept whole because that is what `go install` takes and what
    /// the gate's install command carries, so comparing the two needs no
    /// reassembly.
    fn pinned_go_tools() -> Vec<String> {
        read(".github/go-tools")
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(|line| {
                let (path, version) = line
                    .split_once('@')
                    .unwrap_or_else(|| panic!("{line:?} is not module@version"));
                assert!(!version.is_empty(), "{line:?} has no version");
                assert!(!path.is_empty(), "{line:?} has no module path");
                line.to_string()
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
    /// The `ember-*` path entries are checked like any other. They are
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
            "the skeleton manifests carry ignore lists"
        );
        let mut stale: Vec<String> = Vec::new();
        for (manifest, ignored) in lists {
            let crate_dir = manifest.parent().expect("a manifest has a directory");
            let files = rust_sources(&crate_dir.join("src"));
            for name in ignored {
                let ident = name.replace('-', "_");
                for file in &files {
                    let text = std::fs::read_to_string(file).unwrap_or_default();
                    if references_crate(&text, &ident) {
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

    /// `.github/commit-scopes.json` is the scope vocabulary. `cargo xtask
    /// scopes` prints it and `commitlint.config.js` enforces it, both by reading
    /// the file. `CONTRIBUTING.md` restates it for a reader, and that
    /// restatement is the copy this holds to the file.
    #[test]
    fn every_copy_of_the_scope_list_agrees() {
        let declared = crate::scopes().expect(".github/commit-scopes.json parses");
        assert!(
            !declared.is_empty(),
            ".github/commit-scopes.json declares no scopes"
        );

        let contributing = read("CONTRIBUTING.md");
        let commits = section(&contributing, "Commit messages")
            .expect("CONTRIBUTING.md has a Commit messages section");
        let documented = code_span_lists(&commits);
        assert_eq!(
            documented.len(),
            1,
            "the Commit messages section holds {} paragraphs that are a bare list of code spans, so which one restates the scopes is ambiguous",
            documented.len()
        );
        assert_eq!(
            documented[0], declared,
            "CONTRIBUTING.md restates {:?} and .github/commit-scopes.json holds {declared:?}",
            documented[0]
        );
    }

    /// `commitlint.config.js` with its comment lines dropped.
    fn commitlint_code() -> String {
        read("commitlint.config.js")
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<&str>>()
            .join("\n")
    }

    /// How many times `code` uses `name` as an identifier. A use is the name
    /// with no identifier character or hyphen before it and no identifier
    /// character after it, so a file path holding the name is not one.
    fn identifier_uses(code: &str, name: &str) -> usize {
        let is_ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
        code.match_indices(name)
            .filter(|(at, _)| {
                let before = code[..*at].chars().next_back();
                let after = code[at + name.len()..].chars().next();
                !before.is_some_and(|c| is_ident(c) || c == '-') && !after.is_some_and(is_ident)
            })
            .count()
    }

    /// The commit hook enforces the scope file only while the rule reads it and
    /// nothing else. A literal beside the list, a rule turned down to a
    /// warning or off, or a list extended after it loads each lets commitlint
    /// accept a scope `cargo xtask scopes` never prints.
    ///
    /// The config is JavaScript and nothing in this suite runs JavaScript, so
    /// the rule and the line that loads the list are held to their exact text
    /// with whitespace and trailing commas dropped, which is the part Prettier
    /// rewrites. The list's name has to appear exactly where those two lines
    /// put it, so nothing else in the config can read or change it.
    #[test]
    fn commitlint_enforces_the_scope_file_and_nothing_beside_it() {
        let code = commitlint_code();
        let compact: String = code
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .replace(",]", "]")
            .replace(",)", ")");

        for (what, expected) in [
            ("the rule", r#""scope-enum":[2,"always",scopes]"#),
            (
                "the line that loads the list",
                r#"constscopes=JSON.parse(readFileSync(newURL(".github/commit-scopes.json",import.meta.url),"utf8"))"#,
            ),
        ] {
            assert_eq!(
                compact.matches(expected).count(),
                1,
                "commitlint.config.js does not hold {what} exactly once, as {expected}"
            );
        }
        assert_eq!(
            compact.matches("scope-enum").count(),
            1,
            "commitlint.config.js names scope-enum more than once"
        );
        let uses = identifier_uses(&code, "scopes");
        assert_eq!(
            uses, 2,
            "commitlint.config.js names the list {uses} times, so something beside the rule reads or changes it"
        );
    }

    /// The identifier count reads a name, and neither a path holding it nor a
    /// longer name.
    #[test]
    fn identifier_uses_reads_a_name_and_nothing_holding_it() {
        let cases = [
            ("const scopes = 1;", 1),
            ("[2, \"always\", scopes]", 1),
            ("[...scopes, \"docs\"]", 1),
            ("scopes.push(\"docs\")", 1),
            ("\".github/commit-scopes.json\"", 0),
            ("const allScopes = scopesList;", 0),
            ("", 0),
        ];

        for (code, expected) in cases {
            assert_eq!(identifier_uses(code, "scopes"), expected, "{code}");
        }
    }

    /// `CONTRIBUTING.md` lists the gate's steps for a reader, and that table is
    /// the one copy outside the step table itself. A step added, renamed,
    /// dropped or moved has to move there too.
    #[test]
    fn the_documented_gate_table_names_every_step_in_order() {
        let contributing = read("CONTRIBUTING.md");
        let gate =
            section(&contributing, "The gate").expect("CONTRIBUTING.md has a The gate section");
        let documented = first_column(&gate);
        let declared: Vec<&str> = STEPS.iter().map(|step| step.name).collect();

        assert_eq!(
            documented, declared,
            "the CONTRIBUTING.md gate table and the step table in check.rs disagree"
        );
    }

    /// A count of gate steps or pinned tools in prose goes stale the next time
    /// a step or a tool is added, and nothing else notices. The authoritative
    /// lists are `cargo xtask check`, `.github/cargo-tools` and
    /// `.github/go-tools`, and prose names those rather than counting them.
    ///
    /// Markdown and the issue forms are read whole, code blocks included, and
    /// code, configuration and the workflows contribute their comments, and a
    /// workflow contributes its `name:` values, which the Actions page shows. A
    /// file of a kind the check reads that is not UTF-8 fails it, rather than
    /// being skipped. What passes: a count of anything else, a singular, an
    /// ordinal, a count more than two words from its noun, and code outside a
    /// comment, so a test asserting a rendered summary line is not read as a
    /// claim about the gate.
    #[test]
    fn no_prose_states_a_count_of_gate_steps_or_pinned_tools() {
        let root = workspace_root();
        let mut read_files = 0;
        let mut stated: Vec<String> = Vec::new();
        let mut unreadable: Vec<String> = Vec::new();
        for path in prose_files(&root) {
            if prose_kind(&path).is_none() {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                unreadable.push(
                    path.strip_prefix(&root)
                        .unwrap_or(&path)
                        .display()
                        .to_string(),
                );
                continue;
            };
            read_files += 1;
            let shown = path.strip_prefix(&root).unwrap_or(&path).display();
            for paragraph in prose_paragraphs(&path, &text) {
                for phrase in stated_counts(&paragraph) {
                    stated.push(format!("{shown}: {phrase}"));
                }
            }
        }

        assert!(read_files > 0, "no file was read, so nothing was checked");
        assert!(
            unreadable.is_empty(),
            "these files carry prose and are not UTF-8, so nothing here read them: {unreadable:?}"
        );
        assert!(
            stated.is_empty(),
            "prose states a count of gate steps or pinned tools, which goes stale the next \
             time one is added. Name the command or the file that lists them instead: {stated:#?}"
        );
    }

    /// The phrase shapes the prose check refuses, and the ones it lets through.
    #[test]
    fn stated_counts_reads_the_shapes_it_names() {
        let cases: &[(&str, &[&str])] = &[
            ("Eleven steps, in order", &["eleven steps"]),
            ("The gate calls seven\ntools that rustup", &["seven tools"]),
            ("13 steps passed", &["13 steps"]),
            ("Ember's two extra steps", &["two extra steps"]),
            (
                "the three supply chain steps",
                &["three supply chain steps"],
            ),
            ("an eleven-step gate", &["eleven-step"]),
            ("five pinned tools", &["five pinned tools"]),
            ("Steps: 13", &["steps 13"]),
            ("| Steps | 13 |", &["steps 13"]),
            (
                "the gate's step count is thirteen",
                &["step count is thirteen"],
            ),
            ("all 13 `cargo xtask check` steps pass", &["13 code steps"]),
            ("the six crates.io tools", &["six crates io tools"]),
            ("twenty-one steps", &["twenty-one steps"]),
            ("thirty stages", &["thirty stages"]),
            ("a twenty-one-step gate", &["twenty-one-step"]),
            ("The gate runs thirteen checks", &["thirteen checks"]),
            ("nine commands run in order", &["nine commands"]),
            ("Step 1 installs Rust", &[]),
            ("The gate checks two things", &[]),
            ("The scan steps through `.rdata` eight bytes at a time", &[]),
            ("bun run tools/watch-builds.ts 2278520 appinfo.txt", &[]),
            ("See docs/dev.md. Steps run in order", &[]),
            ("one step at a time", &[]),
            ("the first step", &[]),
            ("It has nine. Steps run in order", &[]),
            ("two of the many tools", &[]),
            ("13 libraries and 344 functions", &[]),
            ("the four before it", &[]),
            ("a three-way merge", &[]),
            ("the steps cargo xtask check runs", &[]),
            ("", &[]),
        ];

        for (prose, expected) in cases {
            assert_eq!(stated_counts(prose), *expected, "{prose:?}");
        }
    }

    /// Markdown reads whole, code contributes its comments alone, and a comment
    /// wrapped across lines reads as one paragraph.
    #[test]
    fn prose_paragraphs_reads_each_kind_of_file() {
        let cases: &[(&str, &str, &[&str])] = &[
            (
                "a.rs",
                "/// runs eleven\n/// steps\nconst NINE: &str = \"nine steps\";\n",
                &["eleven steps"],
            ),
            (
                "a.yml",
                "# seven tools\nrun: echo nine steps\n",
                &["seven tools"],
            ),
            ("a.toml", "key = 1 # five tools\n", &["five tools"]),
            (
                "ci.yml",
                "      - name: Run the eleven gate steps # the gate\n        run: echo nine steps\n",
                &["eleven gate steps"],
            ),
            (
                "a.md",
                "Nine\nsteps.\n\n```text\n13 steps passed\n```\n",
                &["nine steps", "13 steps"],
            ),
            (
                ".github/ISSUE_TEMPLATE/bug.yml",
                "description: the gate's nine steps\n",
                &["nine steps"],
            ),
            ("a.json", "{\"note\": \"nine steps\"}", &[]),
        ];

        for (path, text, expected) in cases {
            let found: Vec<String> = prose_paragraphs(Path::new(path), text)
                .iter()
                .flat_map(|paragraph| stated_counts(paragraph))
                .collect();
            assert_eq!(found, *expected, "{path}");
        }
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
        let tabled = first_column(&crates);

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

    /// The gate names the tool it needs and the workflow installs it, so the
    /// version has to be the same one in both places. A gate asking for one
    /// actionlint while continuous integration installs another is a gate whose
    /// findings nobody can reproduce.
    ///
    /// `cargo_install_package` reads the crates.io half of this. It returns
    /// nothing for a `go install`, which is why this is a check of its own.
    #[test]
    fn the_pinned_go_tool_file_and_the_gate_name_the_same_versions() {
        let pinned = pinned_go_tools();
        assert!(!pinned.is_empty(), "the pinned Go tool file names nothing");

        let installs: Vec<&str> = STEPS
            .iter()
            .map(|step| step.install)
            .filter(|install| install.starts_with("go install "))
            .collect();

        for entry in &pinned {
            let wanted = format!("go install {entry}");
            assert!(
                installs.contains(&wanted.as_str()),
                "no gate step installs {entry}, and the steps that use `go install` are {installs:?}"
            );
        }
        assert_eq!(
            installs.len(),
            pinned.len(),
            "the gate has {} `go install` steps and the pinned file names {}",
            installs.len(),
            pinned.len()
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

    /// A paragraph of prose holding code spans is not a list, a bullet is not a
    /// bare paragraph, a list wrapped across lines still reads, a table yields
    /// its first column alone, and a section ends at the next heading of its
    /// level.
    #[test]
    fn the_markdown_readers_take_the_shapes_they_name() {
        let markdown = "# Title\n\n## First\n\nRun `cargo xtask scopes` for the live list.\n\n\
                        `one`, `two`,\n`three`.\n\n- `four`, `five`\n\n### Inside\n\n`six`.\n\n\
                        ## Second\n\n| Crate | Holds |\n| --- | --- |\n| `alpha` | The first |\n\
                        | `beta` | The second |\n\n## Third\n\n`seven`.\n";

        let first = section(markdown, "First").expect("a First section");
        assert_eq!(
            code_span_lists(&first),
            [vec!["one", "two", "three"], vec!["six"]]
        );
        assert!(
            first_column(&first).is_empty(),
            "the First section holds no table"
        );

        let second = section(markdown, "Second").expect("a Second section");
        assert_eq!(first_column(&second), ["alpha", "beta"]);
        assert!(
            code_span_lists(&second).is_empty(),
            "a table is not a paragraph"
        );

        assert!(section(markdown, "Fourth").is_none());
        assert!(
            section(markdown, "Inside").is_none(),
            "only a level-two heading opens a section"
        );
    }

    /// A crate name ends at a non-identifier character, so one crate whose
    /// name prefixes another does not read as a use of it.
    ///
    /// `serde` and `serde_json` are two crates, and `cargo machete` judges
    /// them separately. A check that read the first inside the second would
    /// demand a prune that makes the machete step fail on the same name, and
    /// both steps are in the gate.
    #[test]
    fn references_crate_ends_the_name_at_a_word_boundary() {
        let cases = [
            ("use serde_json::Value;", "serde", false),
            ("use serde_json::Value;", "serde_json", true),
            ("use serde::Serialize;", "serde", true),
            ("use serde;", "serde", true),
            (
                "pub use ember_enshrouded as game;",
                "ember_enshrouded",
                true,
            ),
            ("#[derive(thiserror::Error)]", "thiserror", true),
            ("extern crate serde;", "serde", true),
            ("/// derives serde::Serialize", "serde", true),
            ("let my_serde = 1;", "serde", false),
            ("use tracing_subscriber::fmt;", "tracing", false),
            ("use tracing_subscriber::fmt;", "tracing_subscriber", true),
            ("// serde is not used here", "serde", false),
            ("", "serde", false),
        ];

        for (text, ident, expected) in cases {
            assert_eq!(
                references_crate(text, ident),
                expected,
                "{text:?} against {ident:?}"
            );
        }
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
