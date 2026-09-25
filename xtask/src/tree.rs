//! The files the gate's tools read before they run, and the waivers in the
//! code they check.
//!
//! A program the gate, its hooks or an install start that finds a config by
//! name, with no flag naming one, has every other name it reads refused here.
//! A config that changes what a row reports is refused on disk, tracked or not,
//! so a local gate agrees with continuous integration. A personal file is
//! refused only when tracked, and `.gitignore` lists it. What a config holds is
//! CODEOWNERS' to review, so the rules here refuse only a key that runs or
//! redirects code from a file that reads as data. The shared `commits` job
//! refuses a `patchedDependencies` key, a `bunfig.toml` key and TypeScript
//! `paths` before a merge, reading the committed tree, and the rules here do
//! not repeat them.
//!
//! `cargo xtask pins`, the gate's opening row, runs these rules before any
//! tool starts. Names compare through [`fold`].

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Stdio;
use std::str::FromStr;

use proc_macro2::{Delimiter, TokenStream, TokenTree};

// ///////////////////////////////////////////////
// Comparing names
// ///////////////////////////////////////////////

/// `name` as the gate compares it: every character mapped to upper case and
/// back to lower case.
///
/// NTFS and APFS open a name in any case, so two names this maps to one string
/// can open one file. The full case mapping takes the long s, the dotless i and
/// the Kelvin sign to `s`, `i` and `k`. APFS also treats canonically equivalent
/// names as one, and the only character canonically equal to a letter a config
/// name holds is the Kelvin sign, which the case mapping already covers.
#[must_use]
pub fn fold(name: &str) -> String {
    name.to_uppercase().to_lowercase()
}

/// Whether `text` matches `pattern`, where `*` is any run of characters that
/// holds no `/`.
fn wildcard(pattern: &[char], text: &[char]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some(('*', rest)) => (0..=text.len())
            .take_while(|&taken| !text[..taken].contains(&'/'))
            .any(|taken| wildcard(rest, &text[taken..])),
        Some((first, rest)) => text
            .split_first()
            .is_some_and(|(head, tail)| head == first && wildcard(rest, tail)),
    }
}

/// Whether the last segment of `path` ends in the extension `wanted`, compared
/// exactly, so a caller folds first where case does not matter.
#[must_use]
pub fn has_extension(path: &str, wanted: &str) -> bool {
    path.rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .is_some_and(|(_, extension)| extension == wanted)
}

/// Whether the folded `path` matches `pattern`: `*` is any run within one
/// segment, and a leading `**/` is any directory, the root included.
fn path_matches(pattern: &str, path: &str) -> bool {
    let (anywhere, body) = match pattern.strip_prefix("**/") {
        Some(body) => (true, body),
        None => (false, pattern),
    };
    let body: Vec<char> = body.chars().collect();
    let text: Vec<char> = path.chars().collect();
    if wildcard(&body, &text) {
        return true;
    }
    anywhere
        && text
            .iter()
            .enumerate()
            .filter(|(_, character)| **character == '/')
            .any(|(at, _)| wildcard(&body, &text[at + 1..]))
}

// ///////////////////////////////////////////////
// The listing
// ///////////////////////////////////////////////

/// The files the rules read: the tracked ones, and the untracked ones on disk.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listing {
    /// Every tracked path, with `/` separators.
    pub tracked: Vec<String>,
    /// Every path on disk that git does not track, ignored ones included,
    /// outside the directories [`UNTRACKED_EXCLUDES`] names.
    pub untracked: Vec<String>,
}

/// The directories the untracked listing leaves out. Each holds build output,
/// installed packages, fetched builds or another worktree, and no tool the gate
/// starts reads a config from one.
const UNTRACKED_EXCLUDES: &[&str] = &[
    "--exclude=node_modules",
    "--exclude=/target/",
    "--exclude=/.cache/",
    "--exclude=/.claude/worktrees/",
];

/// The variables git starts with, and nothing else: no system or global
/// config, so no `core.*` or pathspec setting a contributor keeps changes what
/// it lists.
const GIT_ENV: &[(&str, &str)] = &[
    ("GIT_CONFIG_NOSYSTEM", "1"),
    // Git for Windows reads /dev/null as an empty file too.
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
];

/// The tracked files under `root`, and the untracked ones on disk, from two
/// `git ls-files` calls with an empty environment.
///
/// git never lists a submodule's files, so a checkout nested as a submodule is
/// its own repository's to hold.
///
/// # Errors
///
/// Returns the sentence the pins row carries when git cannot list the tree, or
/// when the work tree git lists is not `root`.
pub fn listing(root: &Path) -> Result<Listing, String> {
    listing_by(root, &|args| git(root, args))
}

/// [`listing`] with `git` answering each git call under `root`.
fn listing_by(
    root: &Path,
    git: &dyn Fn(&[&str]) -> Result<String, String>,
) -> Result<Listing, String> {
    let top = git(&["rev-parse", "--show-toplevel"])?;
    if let Some(finding) = work_tree_finding(top.trim_end_matches(['\r', '\n']), root) {
        return Err(finding);
    }
    let tracked = paths(&git(&["ls-files", "-z"])?);
    let mut others = vec!["ls-files", "-z", "--others"];
    others.extend(UNTRACKED_EXCLUDES);
    let untracked = paths(&git(&others)?);
    Ok(Listing { tracked, untracked })
}

/// Why the listing is refused, when `top`, the work tree `git rev-parse
/// --show-toplevel` names from `root`, is not `root` itself.
///
/// A `.git` that holds no repository, an empty directory among them, sends git
/// up to the repository above, and a `core.worktree` setting points it at
/// another directory. git then lists that work tree's files without a word.
/// Both paths are compared resolved, so no spelling of either decides it.
fn work_tree_finding(top: &str, root: &Path) -> Option<String> {
    let same = match (std::fs::canonicalize(top), std::fs::canonicalize(root)) {
        (Ok(top), Ok(root)) => top == root,
        _ => false,
    };
    (!same).then(|| {
        format!(
            "git names the work tree {top:?}, which is not the root, so the tree rules would read another directory's files. The root's .git holds no repository git can open, or its config points the work tree elsewhere"
        )
    })
}

/// What one git call under `root` printed, with an empty environment.
fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let git = crate::spawn::resolve("git")?;
    let mut command = crate::spawn::command(&git)?;
    command.env_clear().envs(GIT_ENV.iter().copied());
    let output = command
        .current_dir(root)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("git could not list the files the gate refuses: {err}"))?;
    if !output.status.success() {
        return Err(printable(format!(
            "git could not list the files the gate refuses: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The paths a `git ls-files -z` call printed, once each and in order.
fn paths(listed: &str) -> Vec<String> {
    let listed: BTreeSet<&str> = listed.split('\0').filter(|path| !path.is_empty()).collect();
    listed.into_iter().map(str::to_string).collect()
}

// ///////////////////////////////////////////////
// The configs each tool finds by name
// ///////////////////////////////////////////////

/// A program that reads a file it finds by name: the names it reads, and the
/// one the gate names for it.
struct Search {
    /// What a refused file is, read after "is".
    what: &'static str,
    /// Every path the program reads, as a pattern over the folded path.
    paths: &'static [&'static str],
    /// The one path the gate names for the program, which passes in this exact
    /// spelling alone.
    named: Option<&'static str>,
    /// What the program does with a refused file, read after "and".
    reads: &'static str,
    /// True for a file a contributor keeps on their own machine, refused only
    /// when tracked. Every other file is refused on disk, tracked or not.
    personal: bool,
}

/// Every program the gate, its hooks or an install start that reads a file it
/// finds by name with no flag naming one, with every name it reads.
///
/// rustfmt runs with `--config-path rustfmt.toml`, and cargo-deny, taplo,
/// zizmor, Prettier and commitlint each with `--config`. Each of those reads no
/// other config under its flag, a nested one or one above the checkout
/// included, so none appears here. clippy runs under `CLIPPY_CONF_DIR` set to
/// the root and searches upward from there, so the committed root
/// `clippy.toml` stops the search, and `.clippy.toml`, which wins beside it,
/// is refused. cargo, rustup, actionlint and lefthook take no flag at all, so
/// their other names are refused. The patterns reach past each program's own
/// search where that costs nothing, to any directory and any extension. The
/// root `.config` directory, which mise, lefthook, cargo-nextest and
/// commitlint's cosmiconfig read, is refused whole on its own, since
/// cosmiconfig reads its own settings from there even under `--config`.
const SEARCHES: &[Search] = &[
    Search {
        what: "an env file",
        paths: &[
            "**/.env",
            "**/.env.local",
            "**/.env.development",
            "**/.env.development.local",
            "**/.env.production",
            "**/.env.production.local",
            "**/.env.test",
            "**/.env.test.local",
        ],
        named: None,
        reads: "Bun loads one into the environment of every bun run started beside it",
        personal: true,
    },
    Search {
        what: "a local lefthook config",
        paths: &[
            "lefthook-local",
            "lefthook-local.*",
            ".lefthook-local",
            ".lefthook-local.*",
        ],
        named: None,
        reads: "lefthook merges it over lefthook.yml, where it can replace any hook job. .gitignore lists it",
        personal: true,
    },
    Search {
        what: "a second clippy config",
        paths: &[".clippy.toml"],
        named: None,
        reads: "clippy reads it in place of clippy.toml in the directory CLIPPY_CONF_DIR names",
        personal: false,
    },
    Search {
        what: "an actionlint config",
        paths: &[".github/actionlint.yaml", ".github/actionlint.yml"],
        named: None,
        reads: "actionlint reads it, and it can ignore any finding by pattern, ShellCheck's included",
        personal: false,
    },
    Search {
        what: "a lefthook config",
        paths: &["lefthook", "lefthook.*", ".lefthook", ".lefthook.*"],
        named: Some("lefthook.yml"),
        reads: "lefthook reads it in place of lefthook.yml",
        personal: false,
    },
    Search {
        what: "a cargo config",
        paths: &["**/.cargo/config", "**/.cargo/config.toml"],
        named: Some(".cargo/config.toml"),
        reads: "cargo reads one from the directory it starts in and every directory above, and .cargo/config wins beside .cargo/config.toml",
        personal: false,
    },
    Search {
        what: "a toolchain file",
        paths: &["**/rust-toolchain", "**/rust-toolchain.toml"],
        named: Some("rust-toolchain.toml"),
        reads: "rustup picks the toolchain from the nearest one to the directory cargo starts in",
        personal: false,
    },
];

/// The names Bun and TypeScript read a project's options from.
const PROJECT_CONFIG_NAMES: &[&str] = &["tsconfig.json", "jsconfig.json"];

/// The directory GitHub reads workflows from, where actionlint and zizmor read
/// a lowercase `.yml` name alone.
const WORKFLOWS: &str = ".github/workflows";

/// Whether `text` carries an inline zizmor waiver, in any case and spacing
/// zizmor might read.
fn zizmor_waiver(text: &str) -> bool {
    let folded = text.to_ascii_lowercase();
    folded.match_indices("zizmor").any(|(at, _)| {
        let rest = folded[at + "zizmor".len()..].trim_start();
        rest.strip_prefix(':')
            .map(str::trim_start)
            .and_then(|rest| rest.strip_prefix("ignore"))
            .is_some_and(|rest| rest.trim_start().starts_with('['))
    })
}

// ///////////////////////////////////////////////
// JSON read one way
// ///////////////////////////////////////////////

/// Every key that appears twice within one object of `text`, which is JSON
/// that parses.
///
/// Strings are skipped whole, and a key is decoded, so an escaped spelling
/// counts as the key it spells.
fn repeated_keys(text: &str) -> Vec<String> {
    let characters: Vec<char> = text.chars().collect();
    let mut repeated = Vec::new();
    // One entry per open object or array; an array has no keys.
    let mut open: Vec<Option<BTreeSet<String>>> = Vec::new();
    let mut at = 0;
    while at < characters.len() {
        match characters[at] {
            '"' => {
                let mut end = at + 1;
                while end < characters.len() && characters[end] != '"' {
                    end += if characters[end] == '\\' { 2 } else { 1 };
                }
                let token: String = characters[at..=end.min(characters.len() - 1)]
                    .iter()
                    .collect();
                at = end + 1;
                while at < characters.len() && characters[at].is_whitespace() {
                    at += 1;
                }
                if let Some(Some(keys)) = open.last_mut()
                    && characters.get(at) == Some(&':')
                {
                    let key = serde_json::from_str::<String>(&token).unwrap_or(token);
                    if !keys.insert(key.clone()) {
                        repeated.push(key);
                    }
                }
                continue;
            }
            '{' => open.push(Some(BTreeSet::new())),
            '[' => open.push(None),
            '}' | ']' => {
                open.pop();
            }
            _ => {}
        }
        at += 1;
    }
    repeated
}

/// `text` parsed as JSON, refusing a key repeated within one object.
///
/// Bun's `package.json` and `tsconfig.json` reader keeps the first of two equal
/// keys, and `serde_json` the last, so the gate refuses a file with a repeated
/// key rather than read it two ways.
///
/// # Errors
///
/// Returns the sentence saying why, when the text does not parse or repeats a
/// key.
pub fn parse_json(text: &str) -> Result<serde_json::Value, String> {
    let parsed: serde_json::Value =
        serde_json::from_str(text).map_err(|err| format!("it does not parse: {err}"))?;
    let repeated = repeated_keys(text);
    if repeated.is_empty() {
        Ok(parsed)
    } else {
        Err(format!(
            "it repeats {} within one object, and Bun reads the first where a JSON parser reads the last",
            repeated
                .iter()
                .map(|key| format!("{key:?}"))
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }
}

// ///////////////////////////////////////////////
// Inline waivers
// ///////////////////////////////////////////////

/// The lint groups rustc 1.98 lists under `rustc -W help`. A waiver names the
/// exact lint it waives, and a group waives lints nobody named.
const RUSTC_GROUPS: &[&str] = &[
    "warnings",
    "deprecated_safe",
    "future_incompatible",
    "keyword_idents",
    "let_underscore",
    "nonstandard_style",
    "refining_impl_trait",
    "rust_2018_compatibility",
    "rust_2018_idioms",
    "rust_2021_compatibility",
    "rust_2024_compatibility",
    "unknown_or_malformed_diagnostic_attributes",
    "unused",
];

/// Clippy's lint groups, each named under `clippy::`.
const CLIPPY_GROUPS: &[&str] = &[
    "all",
    "pedantic",
    "nursery",
    "restriction",
    "cargo",
    "style",
    "complexity",
    "perf",
    "correctness",
    "suspicious",
];

/// A literal token as the waiver rules read it: a string reduced to `""` when
/// what it spells is blank and `"s"` otherwise, a character to `'c'`, and
/// anything else as written, so no text inside a literal reads as code.
///
/// Blank is holding no letter and no digit once the escapes are decoded, since
/// clippy accepts a reason no reader can see or read.
fn literal(text: &str) -> String {
    let prefix: String = text.chars().take_while(char::is_ascii_alphabetic).collect();
    let rest = &text[prefix.len()..];
    if rest.starts_with('\'') {
        return "'c'".to_string();
    }
    if rest.starts_with('"') || rest.starts_with('#') {
        let content = match (text.find('"'), text.rfind('"')) {
            (Some(first), Some(last)) if last > first => &text[first + 1..last],
            _ => "",
        };
        let spelled = if prefix.contains('r') {
            content.to_string()
        } else {
            unescape(content)
        };
        return if spelled.chars().any(char::is_alphanumeric) {
            "\"s\""
        } else {
            "\"\""
        }
        .to_string();
    }
    text.to_string()
}

/// What the escapes in the body of a string literal that is not raw spell. A
/// malformed escape is kept as written, since the build refuses it anyway.
fn unescape(content: &str) -> String {
    let mut spelled = String::with_capacity(content.len());
    let mut characters = content.chars().peekable();
    while let Some(character) = characters.next() {
        if character != '\\' {
            spelled.push(character);
            continue;
        }
        match characters.next() {
            Some('n') => spelled.push('\n'),
            Some('r') => spelled.push('\r'),
            Some('t') => spelled.push('\t'),
            Some('0') => spelled.push('\0'),
            Some('x') => {
                let hex: String = characters.by_ref().take(2).collect();
                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                    spelled.push(char::from(byte));
                } else {
                    spelled.push_str("\\x");
                    spelled.push_str(&hex);
                }
            }
            Some('u') => {
                let mut code = String::new();
                if characters.next_if_eq(&'{').is_some() {
                    code = characters
                        .by_ref()
                        .take_while(|next| *next != '}')
                        .collect();
                }
                if let Some(decoded) = u32::from_str_radix(&code.replace('_', ""), 16)
                    .ok()
                    .and_then(char::from_u32)
                {
                    spelled.push(decoded);
                } else {
                    spelled.push_str("\\u{");
                    spelled.push_str(&code);
                    spelled.push('}');
                }
            }
            // A line continuation drops the break and the whitespace after it.
            Some('\n') => while characters.next_if(|next| next.is_whitespace()).is_some() {},
            Some(other) => spelled.push(other),
            None => spelled.push('\\'),
        }
    }
    spelled
}

/// `stream` written out with no whitespace, each literal reduced by
/// [`literal`] and each raw identifier in its plain spelling, which names the
/// same path.
fn flatten(stream: TokenStream, out: &mut String) {
    for tree in stream {
        match tree {
            TokenTree::Group(group) => {
                let (open, close) = match group.delimiter() {
                    Delimiter::Parenthesis => ("(", ")"),
                    Delimiter::Brace => ("{", "}"),
                    Delimiter::Bracket => ("[", "]"),
                    Delimiter::None => ("", ""),
                };
                out.push_str(open);
                flatten(group.stream(), out);
                out.push_str(close);
            }
            TokenTree::Ident(ident) => {
                let name = ident.to_string();
                out.push_str(name.strip_prefix("r#").unwrap_or(&name));
            }
            TokenTree::Punct(punct) => out.push(punct.as_char()),
            TokenTree::Literal(token) => out.push_str(&literal(&token.to_string())),
        }
    }
}

/// Whether `tree` is the punctuation `wanted`.
fn is_punct(tree: Option<&TokenTree>, wanted: char) -> bool {
    matches!(tree, Some(TokenTree::Punct(punct)) if punct.as_char() == wanted)
}

/// The plain spelling of `tree` when it is an identifier.
fn ident_name(tree: &TokenTree) -> Option<String> {
    let TokenTree::Ident(ident) = tree else {
        return None;
    };
    let name = ident.to_string();
    Some(
        name.strip_prefix("r#")
            .map_or_else(|| name.clone(), str::to_string),
    )
}

/// Every way the Rust source at `path` waives a lint outside what the gate
/// accepts: a blank reason, a lint group, `rustfmt::skip`, or source the scan
/// never reads.
///
/// clippy's `allow_attributes_without_reason`, denied at the workspace, refuses
/// an `allow` or `expect` with no reason at all, and this reads what it passes.
/// The file is read as Rust tokens, so no comment or literal reads as an
/// attribute. A file that does not tokenize is a finding, and the build refuses
/// it too.
fn waiver_findings(path: &str, text: &str) -> Vec<String> {
    match TokenStream::from_str(text) {
        Ok(stream) => {
            let mut found = Vec::new();
            waiver_walk(path, stream, &mut found);
            found
        }
        Err(err) => vec![format!(
            "{path} does not tokenize as Rust ({err}), so the lint waivers in it are unknown"
        )],
    }
}

/// Every finding against the attributes in `stream`, at any depth outside
/// another attribute, and against every `include!`.
fn waiver_walk(path: &str, stream: TokenStream, found: &mut Vec<String>) {
    let trees: Vec<TokenTree> = stream.into_iter().collect();
    let mut at = 0;
    while at < trees.len() {
        let line = |tree: &TokenTree| tree.span().start().line;
        if is_punct(trees.get(at), '#') {
            let open = if is_punct(trees.get(at + 1), '!') {
                at + 2
            } else {
                at + 1
            };
            if let Some(TokenTree::Group(group)) = trees.get(open)
                && group.delimiter() == Delimiter::Bracket
            {
                let place = format!("{path}:{}", line(&trees[at]));
                found.extend(attribute_findings(&place, group.stream()));
                at = open + 1;
                continue;
            }
        }
        if ident_name(&trees[at]).as_deref() == Some("include") && is_punct(trees.get(at + 1), '!')
        {
            found.push(format!(
                "{path}:{} includes source with include!, and neither the waiver scan nor rustfmt reads the file it names. Make it a module",
                line(&trees[at])
            ));
        }
        if ident_name(&trees[at]).as_deref() == Some("use") {
            let statement: TokenStream = trees[at + 1..]
                .iter()
                .take_while(|tree| !is_punct(Some(tree), ';'))
                .cloned()
                .collect();
            if names(statement, "include") {
                found.push(format!(
                    "{path}:{} imports include, and under any name it reads a file neither the waiver scan nor rustfmt reads. Make it a module",
                    line(&trees[at])
                ));
            }
        }
        if let TokenTree::Group(group) = &trees[at] {
            waiver_walk(path, group.stream(), found);
        }
        at += 1;
    }
}

/// Whether `stream` holds the identifier `wanted` at any depth.
fn names(stream: TokenStream, wanted: &str) -> bool {
    stream.into_iter().any(|tree| match tree {
        TokenTree::Group(group) => names(group.stream(), wanted),
        other => ident_name(&other).as_deref() == Some(wanted),
    })
}

/// One `allow(...)` or `expect(...)` list: which of the two, and each item
/// written out by [`flatten`].
struct WaiverList {
    /// True for `allow`, false for `expect`.
    allow: bool,
    /// The lints and the reason, one item each.
    items: Vec<String>,
}

/// Every `allow(...)` or `expect(...)` list in `stream`, at any depth, so
/// `cfg_attr` reaches the rules too.
fn waiver_lists(stream: TokenStream, lists: &mut Vec<WaiverList>) {
    let trees: Vec<TokenTree> = stream.into_iter().collect();
    for (at, tree) in trees.iter().enumerate() {
        let TokenTree::Group(group) = tree else {
            continue;
        };
        let name = at
            .checked_sub(1)
            .and_then(|before| ident_name(&trees[before]));
        let named = name
            .as_deref()
            .is_some_and(|name| name == "allow" || name == "expect");
        let pathed = at
            .checked_sub(2)
            .is_some_and(|before| is_punct(trees.get(before), ':'));
        if group.delimiter() == Delimiter::Parenthesis && named && !pathed {
            let mut items = Vec::new();
            let mut item = String::new();
            for inner in group.stream() {
                if is_punct(Some(&inner), ',') {
                    items.push(std::mem::take(&mut item));
                } else {
                    flatten(TokenStream::from(inner), &mut item);
                }
            }
            items.push(item);
            lists.push(WaiverList {
                allow: name.as_deref() == Some("allow"),
                items,
            });
        }
        waiver_lists(group.stream(), lists);
    }
}

/// Every way one attribute, its body given as tokens, waives a lint outside
/// what the gate accepts.
fn attribute_findings(at: &str, body: TokenStream) -> Vec<String> {
    let mut flat = String::new();
    flatten(body.clone(), &mut flat);
    let flat = flat.as_str();
    let mut found = Vec::new();
    if flat.contains('$') {
        found.push(format!(
            "{at} is an attribute built from a macro argument, so what it waives is known only after expansion. Write the attribute out"
        ));
    }
    if flat.starts_with("path=") || (flat.starts_with("cfg_attr(") && flat.contains(",path=")) {
        found.push(format!(
            "{at} sets a module's file with #[path], and the waiver scan reads tracked .rs files by name. Move the module to where its name puts it"
        ));
    }
    if flat.contains("rustfmt::skip") || flat.contains("rustfmt_skip") {
        found.push(format!(
            "{at} carries rustfmt::skip, and rustfmt leaves the item unformatted with no reason given. Format it"
        ));
    }
    if flat.contains("allow_attributes") {
        found.push(format!(
            "{at} names allow_attributes or allow_attributes_without_reason, and an attribute naming either can turn off the check that every waiver is an expect with a reason"
        ));
    }
    let mut lists = Vec::new();
    waiver_lists(body, &mut lists);
    if lists.iter().any(|list| list.allow) {
        found.push(format!(
            "{at} is an allow, and a waiver is an expect, which fails the build once its lint stops firing. clippy refuses an allow wherever its cfg holds, and this refuses one on every platform"
        ));
    }
    for item in lists.into_iter().flat_map(|list| list.items) {
        if let Some(reason) = item.strip_prefix("reason=") {
            if reason == "\"\"" {
                found.push(format!(
                    "{at} gives a lint waiver a blank reason, and every waiver says why"
                ));
            }
        } else if RUSTC_GROUPS.contains(&item.as_str())
            || item
                .strip_prefix("clippy::")
                .is_some_and(|group| CLIPPY_GROUPS.contains(&group))
            || item == "rustdoc::all"
        {
            found.push(format!(
                "{at} waives {item}, a lint group, and a waiver names the exact lint it waives"
            ));
        }
    }
    found
}

/// Every cargo-machete ignore list the manifest at `path` carries, which
/// passes each dependency it names without cargo-machete reading for it.
fn machete_findings(path: &str, read: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    let Some(table) = read(path).and_then(|text| toml::from_str::<toml::Table>(&text).ok()) else {
        return Vec::new();
    };
    ["package", "workspace"]
        .into_iter()
        .filter(|section| {
            table
                .get(*section)
                .and_then(|value| value.get("metadata"))
                .and_then(|value| value.get("cargo-machete"))
                .is_some()
        })
        .map(|section| {
            format!(
                "{path:?} carries [{section}.metadata.cargo-machete], and cargo-machete passes every dependency it lists unread. Remove the dependency until code uses it"
            )
        })
        .collect()
}

/// Every way the crate manifest at `path` takes lints of its own rather than
/// the workspace's: its `[lints]` holds `workspace = true` and nothing else.
fn member_lint_findings(path: &str, read: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    let Some(table) = read(path).and_then(|text| toml::from_str::<toml::Table>(&text).ok()) else {
        return vec![format!(
            "{path:?} does not parse as TOML, so its lints are unknown"
        )];
    };
    if !table.contains_key("package") {
        return Vec::new();
    }
    let lints = table.get("lints").and_then(toml::Value::as_table);
    let inherited = lints.is_some_and(|lints| {
        lints.len() == 1 && lints.get("workspace").and_then(toml::Value::as_bool) == Some(true)
    });
    if inherited {
        Vec::new()
    } else {
        vec![format!(
            "{path:?} does not hold [lints] workspace = true alone, and a crate that sets its own lints can turn off what the workspace forbids"
        )]
    }
}

/// The clippy lints the root manifest denies so that every waiver is an
/// `expect` with a reason, and what a waiver can do without each.
const WAIVER_LINTS: &[(&str, &str)] = &[
    (
        "allow_attributes",
        "a waiver can be an allow, which stays silent once its lint stops firing",
    ),
    (
        "allow_attributes_without_reason",
        "a lint waiver can go without a reason",
    ),
];

/// Every way the root manifest falls short of denying each lint in
/// [`WAIVER_LINTS`].
///
/// The level is `deny`, not `forbid`: clap's derive generates an `allow` of
/// `clippy::restriction`, which rustc refuses under a `forbid`. A crate-level
/// `allow` of either lint would turn a `deny` off, so [`attribute_findings`]
/// refuses any attribute naming one.
fn workspace_lint_findings(read: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    let clippy = read("Cargo.toml")
        .and_then(|text| toml::from_str::<toml::Table>(&text).ok())
        .and_then(|table| table.get("workspace")?.get("lints")?.get("clippy").cloned());
    WAIVER_LINTS
        .iter()
        .filter(|(lint, _)| {
            clippy
                .as_ref()
                .and_then(|clippy| clippy.get(lint))
                .and_then(toml::Value::as_str)
                != Some("deny")
        })
        .map(|(lint, without)| {
            format!(
                "Cargo.toml does not set {lint} = \"deny\" under [workspace.lints.clippy], so {without}"
            )
        })
        .collect()
}

// ///////////////////////////////////////////////
// The findings
// ///////////////////////////////////////////////

/// Every way the tree falls outside what the gate expects, as one sentence each.
///
/// `root_names` is every entry at the repository root, `project_configs` the
/// path of every TypeScript project config the repository keeps, and `read`
/// answers a path's contents or `None` when it cannot be read. Nothing here
/// starts a process, so a test drives every rule with a listing and a map.
#[must_use]
pub fn findings(
    listing: &Listing,
    root_names: &[String],
    project_configs: &[&str],
    read: &dyn Fn(&str) -> Option<String>,
) -> Vec<String> {
    let mut found = listing_findings(listing, project_configs, read);
    found.extend(root_findings(root_names));
    found.extend(toolchain_findings(read));
    found.extend(cargo_config_findings(read));
    found.extend(workspace_lint_findings(read));
    found.into_iter().map(printable).collect()
}

/// Every finding against the listed paths: a config a program reads in place
/// of the one the gate names, a project config, a `package.json` that runs
/// code or reads two ways, a tracked path under `node_modules`, and a tracked
/// file outside what the rows read.
///
/// A path under `node_modules` compares folded, so a name NTFS or APFS could
/// open as `node_modules` counts as one.
fn listing_findings(
    listing: &Listing,
    project_configs: &[&str],
    read: &dyn Fn(&str) -> Option<String>,
) -> Vec<String> {
    let tracked: BTreeSet<&str> = listing.tracked.iter().map(String::as_str).collect();
    let mut found = Vec::new();
    let mut modules = Vec::new();
    let untracked = listing
        .untracked
        .iter()
        .filter(|path| !tracked.contains(path.as_str()));
    for path in listing.tracked.iter().chain(untracked) {
        let is_tracked = tracked.contains(path.as_str());
        let segments: Vec<String> = path.split('/').map(fold).collect();
        if segments.iter().any(|segment| segment == "node_modules") {
            if is_tracked {
                modules.push(path.as_str());
            }
            continue;
        }
        let folded = segments.join("/");
        if let Some(search) = SEARCHES.iter().find(|search| {
            (is_tracked || !search.personal)
                && search.named != Some(path.as_str())
                && search
                    .paths
                    .iter()
                    .any(|pattern| path_matches(pattern, &folded))
        }) {
            let remove = if search.personal {
                "Remove it from the index with git rm --cached"
            } else {
                "Remove it"
            };
            found.push(format!(
                "{path:?} is {}, and {}. {remove}",
                search.what, search.reads
            ));
        }
        let base = segments.last().map_or("", String::as_str);
        if PROJECT_CONFIG_NAMES.contains(&base) {
            found.extend(project_config_findings(path, project_configs, read));
        }
        if is_tracked {
            if base == "package.json" {
                found.extend(package_json_findings(path, read));
            }
            if base == "cargo.toml" {
                found.extend(machete_findings(path, read));
                if path != "Cargo.toml" {
                    found.extend(member_lint_findings(path, read));
                }
            }
            if has_extension(base, "rs")
                && let Some(text) = read(path)
            {
                found.extend(waiver_findings(path, &text));
            }
            if TYPESCRIPT
                .iter()
                .any(|extension| has_extension(base, extension))
                && let Some(text) = read(path)
            {
                found.extend(typescript_findings(path, &text));
            }
            found.extend(scope_findings(path, &segments, read));
        }
    }
    if !modules.is_empty() {
        found.push(format!(
            "{} tracked as or under a node_modules directory. bun install keeps what it finds there, the gate and the hooks run each JavaScript tool from it, and Bun resolves an import from the nearest node_modules first. Remove each from the index with git rm -r --cached",
            modules
                .iter()
                .map(|path| format!("{path:?}"))
                .collect::<Vec<_>>()
                .join(", ")
                + if modules.len() == 1 { " is" } else { " are" }
        ));
    }
    found
}

/// Every finding against the project config at `path`: one the repository
/// does not name is refused, and one it names is read along its `extends`
/// chain.
fn project_config_findings(
    path: &str,
    project_configs: &[&str],
    read: &dyn Fn(&str) -> Option<String>,
) -> Vec<String> {
    if !project_configs.contains(&path) {
        return vec![format!(
            "{path:?} is a TypeScript project config the gate does not name, and tsc reads the nearest one to each file while Bun applies its paths and baseUrl to every import below it. Name it in PROJECT_CONFIGS in xtask/src/check.rs, or remove it"
        )];
    }
    extends_findings(path, path, project_configs, read, &mut BTreeSet::new())
}

/// Every way the named project config `file`, reached from `config`, turns
/// tsc's checking off or extends a config the gate does not name.
///
/// `extends` is one path or a list, and every entry must be a named config, so
/// no base file the rule never reads can carry an option the named one lacks.
fn extends_findings(
    config: &str,
    file: &str,
    project_configs: &[&str],
    read: &dyn Fn(&str) -> Option<String>,
    seen: &mut BTreeSet<String>,
) -> Vec<String> {
    if !seen.insert(file.to_string()) {
        return Vec::new();
    }
    let shown = if file == config {
        format!("{config:?}")
    } else {
        format!("{config:?}, through {file:?},")
    };
    let parsed = match read(file).map(|text| parse_json(&text)) {
        Some(Ok(parsed)) => parsed,
        Some(Err(why)) => {
            return vec![format!(
                "{shown} is not plain JSON the gate reads one way: {why}"
            )];
        }
        None => return vec![format!("{shown} cannot be read")],
    };
    let mut found = Vec::new();
    if let Some(options) = parsed
        .get("compilerOptions")
        .and_then(serde_json::Value::as_object)
        && options
            .iter()
            .any(|(key, value)| fold(key) == "nocheck" && value.as_bool() == Some(true))
    {
        found.push(format!(
            "{shown} sets compilerOptions.noCheck, and tsc then reports no type error while it still lists every file, so the typecheck row passes unchecked. Remove it"
        ));
    }
    if let Some(extended) = parsed.get("extends") {
        let targets: Vec<&serde_json::Value> = match extended {
            serde_json::Value::Array(entries) => entries.iter().collect(),
            single => vec![single],
        };
        for target in targets {
            match extended_config(file, target) {
                Some(next) if project_configs.contains(&next.as_str()) => {
                    found.extend(extends_findings(config, &next, project_configs, read, seen));
                }
                _ => found.push(format!(
                    "{shown} extends {target}, and every config a named one extends must itself be named in PROJECT_CONFIGS"
                )),
            }
        }
    }
    found
}

/// The path an `extends` entry in the config at `from` names, relative to the
/// root, when it is a relative path that stays inside the checkout.
fn extended_config(from: &str, target: &serde_json::Value) -> Option<String> {
    let target = target.as_str()?;
    if !(target.starts_with("./") || target.starts_with("../")) {
        return None;
    }
    let mut parts: Vec<&str> = from.split('/').collect();
    parts.pop();
    for part in target.split('/') {
        match part {
            "." | "" => {}
            ".." => {
                parts.pop()?;
            }
            name => parts.push(name),
        }
    }
    let joined = parts.join("/");
    Some(if has_extension(&joined, "json") {
        joined
    } else {
        format!("{joined}.json")
    })
}

/// Every way the tracked `package.json` at `path` runs code or reads two
/// ways.
///
/// commitlint's cosmiconfig reads its own settings from a `cosmiconfig` key
/// before it looks at `--config`, and a `$import` there loads a module. Bun
/// keeps the first of two equal keys where a JSON parser keeps the last, so a
/// check that reads the file as JSON can pass a key Bun applies.
fn package_json_findings(path: &str, read: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    match read(path).map(|text| parse_json(&text)) {
        Some(Ok(parsed)) => parsed
            .get("cosmiconfig")
            .map(|_| {
                format!(
                    "{path:?} carries a cosmiconfig key, and commitlint's cosmiconfig reads its search settings from it even under --config. Remove it"
                )
            })
            .into_iter()
            .collect(),
        Some(Err(why)) => vec![format!(
            "{path:?} is not plain JSON every reader reads one way: {why}"
        )],
        None => Vec::new(),
    }
}

/// Every way the tracked file at `path` falls outside what the rows read: a
/// workflow not named `.github/workflows/<name>.yml` exactly, and an inline
/// zizmor waiver under `.github`.
///
/// The waiver scan reads the bytes, so no `.gitattributes` entry marking the
/// file binary hides a waiver from it.
fn scope_findings(
    path: &str,
    segments: &[String],
    read: &dyn Fn(&str) -> Option<String>,
) -> Vec<String> {
    let mut found = Vec::new();
    let directory = segments[..segments.len() - 1].join("/");
    let base = segments.last().map_or("", String::as_str);
    let workflow =
        directory == WORKFLOWS && (has_extension(base, "yml") || has_extension(base, "yaml"));
    let exact = path
        .strip_prefix(".github/workflows/")
        .and_then(|name| name.rsplit_once('.'))
        .is_some_and(|(_, extension)| extension == "yml");
    if workflow && !exact {
        found.push(format!(
            "{path:?} is a workflow outside {WORKFLOWS}/<name>.yml, and actionlint and zizmor read that spelling alone. Rename it"
        ));
    }
    if segments.first().is_some_and(|first| first == ".github")
        && let Some(text) = read(path)
        && zizmor_waiver(&text)
    {
        found.push(format!(
            "{path:?} carries a zizmor ignore comment, and zizmor waives the audit it names. A waiver is an entry in .github/zizmor.yml, the one config the zizmor row names"
        ));
    }
    found
}

/// Every root entry the gate refuses, in any case: a `.config`, and a
/// `package.yaml`, which cosmiconfig searches for its own settings beside
/// `package.json`.
fn root_findings(root_names: &[String]) -> Vec<String> {
    root_names
        .iter()
        .filter_map(|name| match fold(name).as_str() {
            ".config" => Some(format!(
                "{name:?} is at the root, and mise, lefthook, commitlint's cosmiconfig and cargo-nextest each read a config from it that no row names. Remove it"
            )),
            "package.yaml" => Some(format!(
                "{name:?} is at the root, and commitlint's cosmiconfig reads its search settings from it even under --config. Remove it"
            )),
            _ => None,
        })
        .collect()
}

/// The extensions tsc reads as TypeScript source.
const TYPESCRIPT: &[&str] = &["ts", "tsx", "mts", "cts"];

/// Every comment directive in the tracked TypeScript file at `path` that turns
/// tsc's checking off by dropping errors.
///
/// tsc reads the directives in any case, so the lines compare folded.
fn typescript_findings(path: &str, text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.to_ascii_lowercase();
        let at = format!("{path}:{}", index + 1);
        for directive in ["@ts-nocheck", "@ts-ignore"] {
            if line.contains(directive) {
                found.push(format!(
                    "{at} carries {directive}, and tsc drops the errors it covers. Fix the type instead"
                ));
            }
        }
        if let Some((_, rest)) = line.split_once("@ts-expect-error")
            && !rest.chars().any(char::is_alphanumeric)
        {
            found.push(format!(
                "{at} carries @ts-expect-error with no reason, and every waiver says why"
            ));
        }
    }
    found
}

/// Every way `rust-toolchain.toml` names more than a channel and its
/// components: a `path`, a profile, a target or any other key.
fn toolchain_findings(read: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    const PATH: &str = "rust-toolchain.toml";
    let Some(text) = read(PATH) else {
        return Vec::new();
    };
    let table: toml::Table = match toml::from_str(&text) {
        Ok(table) => table,
        Err(err) => {
            return vec![format!(
                "{PATH} does not parse: {}",
                one_line(&err.to_string())
            )];
        }
    };
    let mut found = Vec::new();
    for key in table.keys().filter(|key| *key != "toolchain") {
        found.push(format!(
            "{PATH} carries {key:?}, and it holds a [toolchain] table alone"
        ));
    }
    for key in table
        .get("toolchain")
        .and_then(toml::Value::as_table)
        .into_iter()
        .flat_map(toml::Table::keys)
        .filter(|key| !["channel", "components"].contains(&key.as_str()))
    {
        found.push(format!(
            "{PATH} [toolchain] carries {key:?}, and it holds channel and components alone. A path, profile or target changes what every cargo row runs with"
        ));
    }
    found
}

/// Every way `.cargo/config.toml` holds more than the `xtask` alias.
///
/// cargo reads it before any row runs. An alias named after a subcommand a row
/// runs, `build.rustflags`, `build.rustdocflags`, `build.rustc-wrapper` and a
/// target runner each change what a cargo row compiles or runs.
fn cargo_config_findings(read: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    const PATH: &str = ".cargo/config.toml";
    let Some(text) = read(PATH) else {
        return Vec::new();
    };
    let table: toml::Table = match toml::from_str(&text) {
        Ok(table) => table,
        Err(err) => {
            return vec![format!(
                "{PATH} does not parse: {}",
                one_line(&err.to_string())
            )];
        }
    };
    let mut found = Vec::new();
    for key in table.keys().filter(|key| *key != "alias") {
        found.push(format!(
            "{PATH} carries {key:?}, and it holds the xtask alias alone. Any other key changes what a cargo row compiles or runs"
        ));
    }
    for key in table
        .get("alias")
        .and_then(toml::Value::as_table)
        .into_iter()
        .flat_map(toml::Table::keys)
        .filter(|key| *key != "xtask")
    {
        found.push(format!(
            "{PATH} [alias] carries {key:?}, and it holds xtask alone. An alias named after a subcommand a row runs replaces what that row runs"
        ));
    }
    found
}

/// `text` on one line, its line breaks written as spaces.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `finding` with every [`hidden`] character written as its escape, so a value
/// read from a committed file reaches no terminal or CI log raw, and no reader
/// sees a sentence reordered or cut short.
#[must_use]
pub fn printable(finding: String) -> String {
    if !finding.chars().any(hidden) {
        return finding;
    }
    finding
        .chars()
        .map(|character| {
            if hidden(character) {
                character.escape_debug().to_string()
            } else {
                character.to_string()
            }
        })
        .collect()
}

/// Whether `char::escape_debug` escapes `character`, a quote or a backslash
/// aside. It escapes every character that changes how the text around it
/// displays without showing itself: a control, a bidirectional or zero-width
/// mark, an ignorable code point, and a line or paragraph separator among them.
#[must_use]
pub fn hidden(character: char) -> bool {
    !matches!(character, '"' | '\'' | '\\') && character.escape_debug().nth(1).is_some()
}

/// The files a sound tree holds for the rules above, for a test to change one
/// thing in.
#[cfg(test)]
pub(crate) fn sound_files() -> Vec<(String, String)> {
    [
        (".prettierrc", "{ \"singleQuote\": true, \"printWidth\": 120 }"),
        (
            "rust-toolchain.toml",
            "[toolchain]\nchannel = \"1.98.1\"\ncomponents = [\"clippy\", \"rustfmt\"]\n",
        ),
        (
            ".cargo/config.toml",
            "[alias]\nxtask = \"run --locked --package xtask --quiet --\"\n",
        ),
        (
            "Cargo.toml",
            "[workspace.lints.clippy]\nallow_attributes = \"deny\"\nallow_attributes_without_reason = \"deny\"\n",
        ),
        (
            "tsconfig.json",
            "{ \"compilerOptions\": { \"strict\": true } }",
        ),
    ]
    .into_iter()
    .map(|(path, text)| (path.to_string(), text.to_string()))
    .collect()
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use super::{
        Listing, findings, fold, has_extension, hidden, listing_by, parse_json, path_matches,
        printable, sound_files, work_tree_finding,
    };

    /// A repository with no TypeScript project config.
    const PLAIN: &[&str] = &[];

    /// A repository keeping one root `tsconfig.json`.
    const TYPED: &[&str] = &["tsconfig.json"];

    /// A tree to run the rules over: listed paths, root names, and file texts
    /// laid over the sound ones.
    struct Tree {
        tracked: Vec<&'static str>,
        untracked: Vec<&'static str>,
        root: Vec<&'static str>,
        files: BTreeMap<String, Option<String>>,
    }

    impl Tree {
        fn new() -> Self {
            Self {
                tracked: Vec::new(),
                untracked: Vec::new(),
                root: Vec::new(),
                files: BTreeMap::new(),
            }
        }

        fn tracked(mut self, path: &'static str) -> Self {
            self.tracked.push(path);
            self
        }

        fn untracked(mut self, path: &'static str) -> Self {
            self.untracked.push(path);
            self
        }

        fn root(mut self, name: &'static str) -> Self {
            self.root.push(name);
            self
        }

        fn file(mut self, path: &str, text: &str) -> Self {
            self.files.insert(path.to_string(), Some(text.to_string()));
            self
        }

        fn missing(mut self, path: &str) -> Self {
            self.files.insert(path.to_string(), None);
            self
        }

        fn run(&self, project_configs: &[&str]) -> Vec<String> {
            let mut texts: BTreeMap<String, Option<String>> = sound_files()
                .into_iter()
                .map(|(path, text)| (path, Some(text)))
                .collect();
            texts.extend(self.files.clone());
            let listing = Listing {
                tracked: self
                    .tracked
                    .iter()
                    .map(|path| (*path).to_string())
                    .collect(),
                untracked: self
                    .untracked
                    .iter()
                    .map(|path| (*path).to_string())
                    .collect(),
            };
            let root: Vec<String> = self.root.iter().map(|name| (*name).to_string()).collect();
            findings(&listing, &root, project_configs, &|path| {
                texts.get(path).cloned().flatten()
            })
        }
    }

    /// Run each case and assert it found exactly the one finding it names.
    fn finds(cases: Vec<(&str, Tree, String)>, project_configs: &[&str]) {
        for (label, tree, finding) in cases {
            assert_eq!(tree.run(project_configs), vec![finding], "{label}");
        }
    }

    /// The finding for a result-changing config named `path`, where `sentence`
    /// is what it is and what its program does with it.
    fn refused(path: &str, sentence: &str) -> String {
        format!("{path:?} is {sentence}. Remove it")
    }

    /// The finding for a tracked personal file.
    fn untrack(path: &str, sentence: &str) -> String {
        format!("{path:?} is {sentence}. Remove it from the index with git rm --cached")
    }

    const CARGO_READS: &str = "a cargo config, and cargo reads one from the directory it starts in and every directory above, and .cargo/config wins beside .cargo/config.toml";
    const TOOLCHAIN_READS: &str = "a toolchain file, and rustup picks the toolchain from the nearest one to the directory cargo starts in";
    const ACTIONLINT_READS: &str = "an actionlint config, and actionlint reads it, and it can ignore any finding by pattern, ShellCheck's included";
    const LEFTHOOK_READS: &str =
        "a lefthook config, and lefthook reads it in place of lefthook.yml";
    const LOCAL_READS: &str = "a local lefthook config, and lefthook merges it over lefthook.yml, where it can replace any hook job. .gitignore lists it";
    const ENV_READS: &str =
        "an env file, and Bun loads one into the environment of every bun run started beside it";

    /// The sound tree meets every rule, so each refusal below changes one thing
    /// from something that passed.
    #[test]
    fn a_sound_tree_meets_every_rule() {
        let tree = Tree::new()
            .tracked("rustfmt.toml")
            .tracked("deny.toml")
            .tracked(".cargo/config.toml")
            .tracked("rust-toolchain.toml")
            .tracked(".taplo.toml")
            .tracked(".prettierrc")
            .tracked("lefthook.yml")
            .tracked("commitlint.config.js")
            .tracked(".github/zizmor.yml")
            .tracked(".github/workflows/ci.yml")
            .tracked("package.json")
            .tracked("crates/a/src/lib.rs")
            .untracked("lefthook-local.yml")
            .untracked(".env")
            .untracked(".npmrc")
            .untracked("target")
            .root(".github")
            .root("config")
            .file("package.json", r#"{ "name": "x", "devDependencies": {} }"#)
            .file(".github/workflows/ci.yml", "on: push\n");
        assert_eq!(tree.run(PLAIN), Vec::<String>::new());
        let typed = Tree::new().tracked("tsconfig.json");
        assert_eq!(typed.run(TYPED), Vec::<String>::new());
    }

    /// A config of a tool the gate starts with its one config named passes at
    /// any depth, since that tool reads no other: rustfmt, clippy, cargo-deny,
    /// taplo, zizmor, Prettier and commitlint.
    #[test]
    fn a_config_the_named_one_stops_passes() {
        let tree = Tree::new()
            .untracked(".commitlintrc.json")
            .tracked("commitlint.config.ts")
            .untracked("crates/a/package.yaml")
            .untracked("crates/a/rustfmt.toml")
            .untracked(".rustfmt.toml")
            .tracked("clippy.toml")
            .untracked("crates/a/.clippy.toml")
            .untracked(".deny.toml")
            .untracked(".cargo/deny.toml")
            .untracked("taplo.toml")
            .untracked("crates/a/.taplo.toml")
            .untracked("zizmor.yml")
            .tracked(".github/zizmor.yaml")
            .untracked(".prettierrc.json")
            .tracked("docs/.prettierrc")
            .untracked("prettier.config.mjs")
            .tracked("package.json")
            .file("package.json", r#"{ "prettier": {}, "commitlint": {} }"#);
        assert_eq!(
            tree.run(PLAIN),
            Vec::<String>::new(),
            "a config the named one stops"
        );
    }

    /// Every name cargo or rustup reads beside the one config the gate keeps is
    /// refused, tracked or untracked, at any depth and in any case, and the kept
    /// one passes in its exact spelling alone.
    #[test]
    fn every_other_name_cargo_and_rustup_read_is_refused() {
        let cases = vec![
            (
                "a .cargo/config beside the named one",
                Tree::new().untracked(".cargo/config"),
                refused(".cargo/config", CARGO_READS),
            ),
            (
                "a crate's .cargo/config.toml",
                Tree::new().tracked("crates/a/.cargo/config.toml"),
                refused("crates/a/.cargo/config.toml", CARGO_READS),
            ),
            (
                "a root rust-toolchain",
                Tree::new().untracked("rust-toolchain"),
                refused("rust-toolchain", TOOLCHAIN_READS),
            ),
            (
                "a crate's rust-toolchain.toml",
                Tree::new().untracked("crates/a/rust-toolchain.toml"),
                refused("crates/a/rust-toolchain.toml", TOOLCHAIN_READS),
            ),
        ];
        finds(cases, PLAIN);
    }

    /// Every name a workflow or hook tool reads in place of its named config is
    /// refused, and a Kelvin sign in a name does not carry it past the fold.
    #[test]
    fn every_other_name_the_other_tools_read_is_refused() {
        let cases = vec![
            (
                "an actionlint config",
                Tree::new().untracked(".github/actionlint.yml"),
                refused(".github/actionlint.yml", ACTIONLINT_READS),
            ),
            (
                "a lefthook.yaml",
                Tree::new().untracked("lefthook.yaml"),
                refused("lefthook.yaml", LEFTHOOK_READS),
            ),
            (
                "a .lefthook.yml",
                Tree::new().untracked(".lefthook.yml"),
                refused(".lefthook.yml", LEFTHOOK_READS),
            ),
            (
                "a root .clippy.toml",
                Tree::new().untracked(".clippy.toml"),
                refused(
                    ".clippy.toml",
                    "a second clippy config, and clippy reads it in place of clippy.toml in the directory CLIPPY_CONF_DIR names",
                ),
            ),
            (
                "a Kelvin sign in lefthook.yml",
                Tree::new().untracked("lefthoo\u{212A}.yml"),
                refused("lefthoo\u{212A}.yml", LEFTHOOK_READS),
            ),
        ];
        finds(cases, PLAIN);
    }

    /// A personal file is refused when tracked, at any depth, and passes on
    /// disk untracked.
    #[test]
    fn a_personal_file_is_refused_only_when_tracked() {
        let cases = vec![
            (
                "a tracked local lefthook config",
                Tree::new().tracked("lefthook-local.yml"),
                untrack("lefthook-local.yml", LOCAL_READS),
            ),
            (
                "a tracked .lefthook-local",
                Tree::new().tracked(".lefthook-local"),
                untrack(".lefthook-local", LOCAL_READS),
            ),
            (
                "a tracked root env file",
                Tree::new().tracked(".env"),
                untrack(".env", ENV_READS),
            ),
            (
                "a tracked nested env file",
                Tree::new().tracked("crates/a/.env.local"),
                untrack("crates/a/.env.local", ENV_READS),
            ),
            (
                "a tracked env file for a mode, local",
                Tree::new().tracked("tools/.env.test.local"),
                untrack("tools/.env.test.local", ENV_READS),
            ),
            (
                "a tracked env file in another case",
                Tree::new().tracked(".ENV.Production"),
                untrack(".ENV.Production", ENV_READS),
            ),
        ];
        finds(cases, PLAIN);
        let untracked = Tree::new()
            .untracked("lefthook-local.yml")
            .untracked(".lefthook-local.json")
            .untracked("crates/a/.env.local");
        assert_eq!(untracked.run(PLAIN), Vec::<String>::new());
        for name in [
            ".env.example",
            "tools/.env.sample",
            ".env.staging",
            ".npmrc",
        ] {
            let tracked = Tree::new().tracked(name);
            assert_eq!(
                tracked.run(PLAIN),
                Vec::<String>::new(),
                "{name} is no file Bun loads, so it passes tracked"
            );
        }
    }

    /// A tracked path as or under `node_modules`, in any case or folded
    /// spelling, is one finding naming each, and no other rule reads it. An
    /// untracked one is left to the install.
    #[test]
    fn a_tracked_node_modules_path_is_refused() {
        let reads = "tracked as or under a node_modules directory. bun install keeps what it finds there, the gate and the hooks run each JavaScript tool from it, and Bun resolves an import from the nearest node_modules first. Remove each from the index with git rm -r --cached";
        let cases = vec![
            (
                "one path",
                Tree::new().tracked("node_modules/x/index.js"),
                format!("\"node_modules/x/index.js\" is {reads}"),
            ),
            (
                "two paths, one in capitals",
                Tree::new()
                    .tracked("node_modules/.bin/prettier")
                    .tracked("a/NODE_MODULES"),
                format!("\"node_modules/.bin/prettier\", \"a/NODE_MODULES\" are {reads}"),
            ),
            (
                "a long s that folds to node_modules",
                Tree::new().tracked("node_module\u{17f}/.bin/prettier"),
                format!("\"node_module\u{17f}/.bin/prettier\" is {reads}"),
            ),
            (
                "a config name under node_modules draws this finding alone",
                Tree::new().tracked("node_modules/x/rust-toolchain"),
                format!("\"node_modules/x/rust-toolchain\" is {reads}"),
            ),
        ];
        finds(cases, PLAIN);
        let untracked = Tree::new()
            .untracked("node_modules/x/index.js")
            .untracked("node_modules/y/lefthook.yaml");
        assert_eq!(untracked.run(PLAIN), Vec::<String>::new());
    }

    /// A project config the repository does not name is refused on disk at any
    /// depth, and a named one may not turn tsc's checking off or extend a file
    /// the repository does not name.
    #[test]
    fn a_project_config_is_named_and_keeps_checking_on() {
        let unheld = "is a TypeScript project config the gate does not name, and tsc reads the nearest one to each file while Bun applies its paths and baseUrl to every import below it. Name it in PROJECT_CONFIGS in xtask/src/check.rs, or remove it";
        finds(
            vec![
                (
                    "a root tsconfig with none held",
                    Tree::new().untracked("tsconfig.json"),
                    format!("\"tsconfig.json\" {unheld}"),
                ),
                (
                    "a nested jsconfig",
                    Tree::new().untracked("tools/sub/JSConfig.json"),
                    format!("\"tools/sub/JSConfig.json\" {unheld}"),
                ),
            ],
            PLAIN,
        );
        let unchecked = |shown: &str| {
            format!(
                "{shown} sets compilerOptions.noCheck, and tsc then reports no type error while it still lists every file, so the typecheck row passes unchecked. Remove it"
            )
        };
        let extends = r#"{ "extends": "./base", "compilerOptions": { "strict": true } }"#;
        // The base exists and turns nothing off, so only the rule that every
        // extended config is named refuses it.
        let tree = |text: &str| {
            Tree::new()
                .tracked("tsconfig.json")
                .tracked("base")
                .file("tsconfig.json", text)
                .file("base", r#"{ "compilerOptions": {} }"#)
        };
        for (label, text, wanted) in [
            (
                "noCheck",
                r#"{ "compilerOptions": { "noCheck": true } }"#,
                unchecked("\"tsconfig.json\""),
            ),
            (
                "an unnamed base",
                extends,
                "\"tsconfig.json\" extends \"./base\", and every config a named one extends must itself be named in PROJECT_CONFIGS".to_string(),
            ),
            (
                "a repeated key",
                r#"{ "compilerOptions": { "strict": true }, "compilerOptions": { "strict": false } }"#,
                "\"tsconfig.json\" is not plain JSON the gate reads one way: it repeats \"compilerOptions\" within one object, and Bun reads the first where a JSON parser reads the last".to_string(),
            ),
        ] {
            assert_eq!(tree(text).run(TYPED), vec![wanted], "{label}");
        }
        let broken = tree("{ \"compilerOptions\": ").run(TYPED);
        assert_eq!(broken.len(), 1, "{broken:?}");
        assert!(
            broken[0].starts_with(
                "\"tsconfig.json\" is not plain JSON the gate reads one way: it does not parse: "
            ),
            "{broken:?}"
        );
        let named_base = Tree::new()
            .tracked("tsconfig.json")
            .tracked("base.json")
            .file("tsconfig.json", extends)
            .file("base.json", r#"{ "compilerOptions": { "noCheck": true } }"#)
            .run(&["tsconfig.json", "base.json"]);
        assert_eq!(
            named_base,
            vec![unchecked("\"tsconfig.json\", through \"base.json\",")],
            "a named base is read along the chain"
        );
    }

    /// A root and the directory above it, on disk, for the work tree cases.
    fn nested() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = dir.path().join("child");
        std::fs::create_dir(&root).expect("the root directory");
        (dir, root)
    }

    /// `path` as git prints it, with `/` separators.
    fn slashed(path: &Path) -> String {
        path.display().to_string().replace('\\', "/")
    }

    /// A work tree git names other than the root is refused, whatever the
    /// spelling, and the root itself passes in either spelling.
    #[test]
    fn a_work_tree_other_than_the_root_is_refused() {
        let (dir, root) = nested();
        let above = slashed(dir.path());
        let refused = |top: &str| {
            format!(
                "git names the work tree {top:?}, which is not the root, so the tree rules would read another directory's files. The root's .git holds no repository git can open, or its config points the work tree elsewhere"
            )
        };
        let missing = slashed(&dir.path().join("elsewhere"));
        for (what, top, wanted) in [
            ("the root, as git prints it", slashed(&root), None),
            (
                "the root, spelled natively",
                root.display().to_string(),
                None,
            ),
            ("the repository above", above.clone(), Some(refused(&above))),
            (
                "the root, reached through its parent",
                format!("{above}/child/../child"),
                None,
            ),
            (
                "a work tree that is not there",
                missing.clone(),
                Some(refused(&missing)),
            ),
        ] {
            assert_eq!(work_tree_finding(&top, &root), wanted, "{what}");
        }
    }

    /// The listing asks git for its work tree first, and lists nothing when
    /// the answer is not the root.
    #[test]
    fn the_listing_stops_before_git_lists_another_work_tree() {
        let (dir, root) = nested();
        for (what, top, refused) in [
            ("the repository above", slashed(dir.path()), true),
            ("the root", slashed(&root), false),
        ] {
            let asked = RefCell::new(Vec::new());
            let git = |args: &[&str]| {
                asked.borrow_mut().push(args.join(" "));
                Ok(if args[0] == "rev-parse" {
                    format!("{top}\n")
                } else {
                    "b.rs\0a.rs\0".to_string()
                })
            };
            let listed = listing_by(&root, &git);
            let asked = asked.into_inner();
            if refused {
                assert!(listed.is_err(), "{what}: {listed:?}");
                assert_eq!(asked, ["rev-parse --show-toplevel"], "{what}");
            } else {
                let listed = listed.expect("the root lists");
                assert_eq!(listed.tracked, ["a.rs", "b.rs"], "{what}");
                assert_eq!(asked.len(), 3, "{what}: {asked:?}");
            }
        }
    }

    /// A tracked `package.json` that runs code or does not read one way is
    /// refused: a `cosmiconfig` key, a key repeated within one object at any
    /// depth, or text that does not parse. An untracked one is the install's.
    #[test]
    fn a_package_json_that_runs_code_or_reads_two_ways_is_refused() {
        let two_ways = "is not plain JSON every reader reads one way: it repeats";
        let cosmiconfig = "carries a cosmiconfig key, and commitlint's cosmiconfig reads its search settings from it even under --config. Remove it";
        let cases = vec![
            (
                "a cosmiconfig key",
                Tree::new().tracked("package.json").file(
                    "package.json",
                    r#"{ "cosmiconfig": { "$import": ["./probe.mjs"] } }"#,
                ),
                format!("\"package.json\" {cosmiconfig}"),
            ),
            (
                "a nested cosmiconfig key",
                Tree::new()
                    .tracked("a/package.json")
                    .file("a/package.json", r#"{ "cosmiconfig": {} }"#),
                format!("\"a/package.json\" {cosmiconfig}"),
            ),
            (
                "a repeated key",
                Tree::new()
                    .tracked("package.json")
                    .file("package.json", r#"{ "name": "a", "name": "b" }"#),
                format!("\"package.json\" {two_ways} \"name\" within one object, and Bun reads the first where a JSON parser reads the last"),
            ),
            (
                "a nested package repeating a key the shared job refuses",
                Tree::new().tracked("tools/package.json").file(
                    "tools/package.json",
                    r#"{ "patchedDependencies": { "a@1.0.0": "p.patch" }, "patchedDependencies": {} }"#,
                ),
                format!("\"tools/package.json\" {two_ways} \"patchedDependencies\" within one object, and Bun reads the first where a JSON parser reads the last"),
            ),
        ];
        finds(cases, PLAIN);
        let broken = Tree::new()
            .tracked("package.json")
            .file("package.json", "{ \"name\": ")
            .run(PLAIN);
        assert_eq!(broken.len(), 1, "{broken:?}");
        assert!(
            broken[0].starts_with(
                "\"package.json\" is not plain JSON every reader reads one way: it does not parse: "
            ),
            "{broken:?}"
        );
        let untracked = Tree::new()
            .untracked("package.json")
            .file("package.json", r#"{ "name": "a", "name": "b" }"#);
        assert_eq!(untracked.run(PLAIN), Vec::<String>::new());
        let plain = Tree::new().tracked("package.json").file(
            "package.json",
            r#"{ "name": "a", "x": { "cosmiconfig": 1 } }"#,
        );
        assert_eq!(
            plain.run(PLAIN),
            Vec::<String>::new(),
            "a cosmiconfig key below the top level is no setting cosmiconfig reads"
        );
    }

    /// A tracked file outside what the rows read is refused: a workflow named
    /// anything but `<name>.yml`, and an inline zizmor waiver under `.github`.
    #[test]
    fn a_tracked_file_outside_the_rows_is_refused() {
        let workflow = |path: &str| {
            format!(
                "{path:?} is a workflow outside .github/workflows/<name>.yml, and actionlint and zizmor read that spelling alone. Rename it"
            )
        };
        let waiver = |path: &str| {
            format!(
                "{path:?} carries a zizmor ignore comment, and zizmor waives the audit it names. A waiver is an entry in .github/zizmor.yml, the one config the zizmor row names"
            )
        };
        let ci = ".github/workflows/ci.yml";
        let cases = vec![
            (
                "an inline waiver",
                Tree::new().tracked(ci).file(
                    ci,
                    "    secrets: inherit # zizmor: ignore[secrets-inherit]\n",
                ),
                waiver(ci),
            ),
            (
                "a waiver in another spelling",
                Tree::new()
                    .tracked(ci)
                    .file(ci, "# ZIZMOR :IGNORE [unpinned-uses]\n"),
                waiver(ci),
            ),
            (
                "a waiver in dependabot.yml",
                Tree::new().tracked(".github/dependabot.yml").file(
                    ".github/dependabot.yml",
                    "# zizmor: ignore[dependabot-cooldown]\n",
                ),
                waiver(".github/dependabot.yml"),
            ),
            (
                "a workflow in capitals",
                Tree::new()
                    .tracked(".github/workflows/ci.YML")
                    .file(".github/workflows/ci.YML", "on: push\n"),
                workflow(".github/workflows/ci.YML"),
            ),
            (
                "a .yaml workflow",
                Tree::new()
                    .tracked(".github/workflows/ci.yaml")
                    .file(".github/workflows/ci.yaml", "on: push\n"),
                workflow(".github/workflows/ci.yaml"),
            ),
        ];
        finds(cases, PLAIN);
        let elsewhere = Tree::new()
            .tracked("docs/zizmor.md")
            .file("docs/zizmor.md", "a zizmor: ignore[x] comment in prose\n")
            .tracked(".github/workflows/sub/x.yaml")
            .tracked("docs/.jj/x.md")
            .tracked("tools/g.d.ts");
        assert_eq!(elsewhere.run(PLAIN), Vec::<String>::new());
    }

    /// A root `.config` or `package.yaml`, in any case, is refused whole.
    #[test]
    fn a_root_config_directory_or_package_yaml_is_refused() {
        let reads = "is at the root, and mise, lefthook, commitlint's cosmiconfig and cargo-nextest each read a config from it that no row names. Remove it";
        let yaml = "is at the root, and commitlint's cosmiconfig reads its search settings from it even under --config. Remove it";
        let cases = vec![
            (
                "lower case",
                Tree::new().root(".config"),
                format!("\".config\" {reads}"),
            ),
            (
                "upper case",
                Tree::new().root(".CONFIG"),
                format!("\".CONFIG\" {reads}"),
            ),
            (
                "a package.yaml",
                Tree::new().root("package.yaml"),
                format!("\"package.yaml\" {yaml}"),
            ),
            (
                "a package.yaml in capitals",
                Tree::new().root("Package.YAML"),
                format!("\"Package.YAML\" {yaml}"),
            ),
        ];
        finds(cases, PLAIN);
        for name in ["'", "package.yml", "package.json"] {
            assert_eq!(
                Tree::new().root(name).run(PLAIN),
                Vec::<String>::new(),
                "{name} is no name cosmiconfig searches for its own settings"
            );
        }
    }

    /// A tracked TypeScript file cannot turn tsc's checking off: no
    /// `@ts-nocheck` or `@ts-ignore` in any case, and an `@ts-expect-error` only
    /// with a reason.
    #[test]
    fn a_typescript_waiver_is_refused() {
        let source = |path: &'static str, text: &str| Tree::new().tracked(path).file(path, text);
        let directive = |at: &str, name: &str| {
            format!("{at} carries {name}, and tsc drops the errors it covers. Fix the type instead")
        };
        let cases = vec![
            (
                "nocheck",
                source("tools/a.ts", "// @ts-nocheck\nconst x: number = 'a';\n"),
                directive("tools/a.ts:1", "@ts-nocheck"),
            ),
            (
                "ignore in another case",
                source("tools/a.mts", "const y = 1;\n/* @TS-Ignore */\n"),
                directive("tools/a.mts:2", "@ts-ignore"),
            ),
            (
                "a bare expect-error",
                source("tools/a.tsx", "// @ts-expect-error\n"),
                "tools/a.tsx:1 carries @ts-expect-error with no reason, and every waiver says why"
                    .to_string(),
            ),
            (
                "an expect-error with a zero-width reason",
                source("tools/a.ts", "// @ts-expect-error \u{200B}\n"),
                "tools/a.ts:1 carries @ts-expect-error with no reason, and every waiver says why"
                    .to_string(),
            ),
            (
                "an expect-error with only a colon",
                source("tools/a.cts", "/* @ts-expect-error: */\n"),
                "tools/a.cts:1 carries @ts-expect-error with no reason, and every waiver says why"
                    .to_string(),
            ),
        ];
        finds(cases, PLAIN);
        let reasoned = source(
            "tools/a.ts",
            "// @ts-expect-error the fixture is a malformed record\nparse(1);\n",
        );
        assert_eq!(
            reasoned.run(PLAIN),
            Vec::<String>::new(),
            "a reasoned waiver"
        );
    }

    /// `rust-toolchain.toml` names a channel and its components and nothing
    /// else, whatever values those take.
    #[test]
    fn the_toolchain_file_names_a_channel_and_components_alone() {
        let path = "rust-toolchain.toml";
        let cases = vec![
            (
                "a profile",
                Tree::new().file(path, "[toolchain]\nchannel = \"1.98.1\"\ncomponents = [\"clippy\", \"rustfmt\"]\nprofile = \"complete\"\n"),
                "rust-toolchain.toml [toolchain] carries \"profile\", and it holds channel and components alone. A path, profile or target changes what every cargo row runs with".to_string(),
            ),
            (
                "a path",
                Tree::new().file(path, "[toolchain]\npath = \"../toolchain\"\n"),
                "rust-toolchain.toml [toolchain] carries \"path\", and it holds channel and components alone. A path, profile or target changes what every cargo row runs with".to_string(),
            ),
            (
                "a table beside the toolchain",
                Tree::new().file(path, "[toolchain]\nchannel = \"1.98.1\"\ncomponents = [\"clippy\", \"rustfmt\"]\n[other]\nx = 1\n"),
                "rust-toolchain.toml carries \"other\", and it holds a [toolchain] table alone".to_string(),
            ),
        ];
        finds(cases, PLAIN);
        for (label, text) in [
            (
                "Renovate's bump",
                "[toolchain]\nchannel = \"1.99.0\"\ncomponents = [\"clippy\", \"rustfmt\"]\n",
            ),
            ("a moving channel", "[toolchain]\nchannel = \"stable\"\n"),
            (
                "another component",
                "[toolchain]\nchannel = \"1.98.1\"\ncomponents = [\"clippy\", \"rustfmt\", \"miri\"]\n",
            ),
        ] {
            assert_eq!(
                Tree::new().file(path, text).run(PLAIN),
                Vec::<String>::new(),
                "{label}"
            );
        }
        assert_eq!(
            Tree::new().missing(path).run(PLAIN),
            Vec::<String>::new(),
            "no toolchain file"
        );
    }

    /// `.cargo/config.toml` holds the xtask alias and nothing else, whatever
    /// that alias runs.
    #[test]
    fn the_cargo_config_holds_the_xtask_alias_alone() {
        let path = ".cargo/config.toml";
        let alias = "[alias]\nxtask = \"run --locked --package xtask --quiet --\"\n";
        let key = |key: &str| {
            format!(
                ".cargo/config.toml carries \"{key}\", and it holds the xtask alias alone. Any other key changes what a cargo row compiles or runs"
            )
        };
        let aliased = |name: &str| {
            format!(
                ".cargo/config.toml [alias] carries \"{name}\", and it holds xtask alone. An alias named after a subcommand a row runs replaces what that row runs"
            )
        };
        let cases = vec![
            (
                "an alias for a row's subcommand",
                Tree::new().file(path, &format!("{alias}fmt = \"run -p x\"\n")),
                aliased("fmt"),
            ),
            (
                "a nextest alias",
                Tree::new().file(path, &format!("{alias}nextest = \"test\"\n")),
                aliased("nextest"),
            ),
            (
                "doc flags that filter every example",
                Tree::new().file(
                    path,
                    &format!("{alias}[build]\nrustdocflags = [\"--test-args=x\"]\n"),
                ),
                key("build"),
            ),
            (
                "a target runner",
                Tree::new().file(
                    path,
                    &format!("{alias}[target.x86_64-pc-windows-msvc]\nrunner = \"a.exe\"\n"),
                ),
                key("target"),
            ),
            (
                "an environment table",
                Tree::new().file(path, &format!("{alias}[env]\nRUST_LOG = \"off\"\n")),
                key("env"),
            ),
        ];
        finds(cases, PLAIN);
        assert_eq!(
            Tree::new()
                .file(path, "[alias]\nxtask = \"run -p xtask --\"\n")
                .run(PLAIN),
            Vec::<String>::new(),
            "the alias running something else"
        );
        assert_eq!(
            Tree::new().missing(path).run(PLAIN),
            Vec::<String>::new(),
            "no cargo config"
        );
        let broken = Tree::new().file(path, "[alias\n").run(PLAIN);
        assert!(
            broken.len() == 1 && broken[0].starts_with(".cargo/config.toml does not parse: "),
            "{broken:?}"
        );
    }

    /// A control character read from the tree reaches the finding escaped, and
    /// so does a bidirectional mark read from a key.
    #[test]
    fn a_finding_carries_no_raw_control_character() {
        let found = Tree::new().untracked("rust-toolchain\u{1b}[2J").run(PLAIN);
        assert!(
            found.is_empty(),
            "the name is not rust-toolchain: {found:?}"
        );
        let found = Tree::new()
            .untracked("a\u{1b}[2J/rust-toolchain")
            .run(PLAIN);
        assert_eq!(
            found,
            vec![refused("a\u{1b}[2J/rust-toolchain", TOOLCHAIN_READS)]
        );
        assert!(!found[0].contains('\u{1b}'), "{found:?}");
        let found = Tree::new()
            .file(
                ".cargo/config.toml",
                "\"\u{202E}x\" = 1\n[alias]\nxtask = \"run --locked --package xtask --quiet --\"\n",
            )
            .run(PLAIN);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(!found[0].contains('\u{202E}'), "{found:?}");
        assert!(found[0].contains("\\u{202e}x"), "{found:?}");
    }

    /// A finding escapes every character `char::escape_debug` escapes, and
    /// leaves the quotes and backslashes of the sentence around a value alone.
    #[test]
    fn a_finding_escapes_what_escape_debug_escapes() {
        for character in [
            '\u{1b}',
            '\n',
            '\u{202E}',
            '\u{200B}',
            '\u{2028}',
            '\u{FEFF}',
            '\u{AD}',
            '\u{E0001}',
        ] {
            assert!(hidden(character), "{character:?}");
        }
        for character in ['"', '\'', '\\', 'a', ' ', '\u{e9}'] {
            assert!(!hidden(character), "{character:?}");
        }
        assert_eq!(
            printable("\"C:\\a\" holds \u{202E}x and\u{1b}[2J".to_string()),
            "\"C:\\a\" holds \\u{202e}x and\\u{1b}[2J",
            "a finding printed with its hidden characters escaped"
        );
    }

    /// The fold maps case both ways, as NTFS and APFS compare a name, and keeps
    /// every other character.
    #[test]
    fn the_fold_merges_what_a_filesystem_merges() {
        for (name, folded) in [
            ("CLIPPY.TOML", "clippy.toml"),
            ("clip\u{200B}py.toml", "clip\u{200B}py.toml"),
            ("\u{FEFF}Deny.toml", "\u{FEFF}deny.toml"),
            ("lefthoo\u{212A}.yml", "lefthook.yml"),
            ("\u{17F}ettings", "settings"),
            ("Tsconf\u{131}g.json", "tsconfig.json"),
            (".con\u{FB01}g", ".config"),
        ] {
            assert_eq!(fold(name), folded, "{name:?}");
        }
    }

    /// A pattern's `*` stays inside one segment, and a leading `**/` matches
    /// at the root and at any depth.
    #[test]
    fn a_pattern_matches_the_paths_it_names() {
        for (pattern, path, matches) in [
            ("**/clippy.toml", "clippy.toml", true),
            ("**/clippy.toml", "a/b/clippy.toml", true),
            ("**/clippy.toml", "a/xclippy.toml", false),
            ("lefthook.*", "lefthook.yml", true),
            ("lefthook.*", "a/lefthook.yml", false),
            ("lefthook.*", "lefthook-local.yml", false),
            ("**/.env*", ".env", true),
            ("**/.env*", "a/.env.local", true),
            ("**/.env*", "a/.envx/b", false),
            ("**/.cargo/config", "a/.cargo/config", true),
            ("**/.cargo/config", ".cargo/config.toml", false),
        ] {
            assert_eq!(
                path_matches(pattern, path),
                matches,
                "{pattern} over {path}"
            );
        }
    }

    /// An extension compares on the last segment alone and exactly.
    #[test]
    fn an_extension_is_the_last_segments_alone() {
        assert!(has_extension("a/b.toml", "toml"));
        assert!(!has_extension("a.toml/b", "toml"));
        assert!(!has_extension("a/b.TOML", "toml"));
        assert!(!has_extension("toml", "toml"));
    }

    /// A lint waiver names the exact lint it waives and gives a reason, in any
    /// attribute form, and nothing inside a comment or a string literal reads as
    /// one.
    #[test]
    fn a_lint_waiver_names_its_lint_and_a_reason() {
        let group = |line: usize, item: &str| {
            format!(
                "crates/a/src/lib.rs:{line} waives {item}, a lint group, and a waiver names the exact lint it waives"
            )
        };
        let blank = |line: usize| {
            format!(
                "crates/a/src/lib.rs:{line} gives a lint waiver a blank reason, and every waiver says why"
            )
        };
        let skip = |line: usize| {
            format!(
                "crates/a/src/lib.rs:{line} carries rustfmt::skip, and rustfmt leaves the item unformatted with no reason given. Format it"
            )
        };
        let names = "crates/a/src/lib.rs:1 names allow_attributes or allow_attributes_without_reason, and an attribute naming either can turn off the check that every waiver is an expect with a reason".to_string();
        let source = |text: &str| {
            Tree::new()
                .tracked("crates/a/src/lib.rs")
                .file("crates/a/src/lib.rs", text)
        };
        let cases = vec![
            (
                "a clippy group",
                source("#[expect(clippy::all, reason = \"x\")]\nfn f() {}\n"),
                group(1, "clippy::all"),
            ),
            (
                "a rustc group at the crate",
                source("#![expect(unused, reason = \"x\")]\n"),
                group(1, "unused"),
            ),
            (
                "a group under cfg_attr",
                source(
                    "fn f() {}\n#[cfg_attr(test, expect(clippy::pedantic, reason = \"x\"))]\nfn g() {}\n",
                ),
                group(2, "clippy::pedantic"),
            ),
            (
                "an empty reason",
                source("#[expect(dead_code, reason = \"\")]\nfn f() {}\n"),
                blank(1),
            ),
            (
                "a whitespace reason",
                source("#[expect(dead_code, reason = \"   \")]\nfn f() {}\n"),
                blank(1),
            ),
            (
                "rustfmt::skip",
                source("#[rustfmt::skip]\nfn f() {}\n"),
                skip(1),
            ),
            (
                "rustfmt::skip spaced out",
                source("#[rustfmt :: skip :: macros(x)]\nfn f() {}\n"),
                skip(1),
            ),
            (
                "the reason check itself",
                source("#![expect(clippy::allow_attributes_without_reason, reason = \"x\")]\n"),
                names.clone(),
            ),
            (
                "the expect check itself",
                source("#![expect(clippy::allow_attributes, reason = \"x\")]\n"),
                names,
            ),
            (
                "a waiver after a lifetime",
                source(
                    "fn f<'a>(x: &'a str) -> &'a str { x }\n#[expect(warnings, reason = \"x\")]\nfn g() {}\n",
                ),
                group(2, "warnings"),
            ),
            (
                "a waiver after a quote character",
                source(
                    r#"const Q: char = '"';
#[expect(unused, reason = "x")]
fn f() {}
"#,
                ),
                group(2, "unused"),
            ),
        ];
        finds(cases, PLAIN);
    }

    /// Nothing inside a comment, a doc comment or a literal reads as a waiver,
    /// and a waiver naming its lint with a reason passes.
    #[test]
    fn no_comment_or_literal_reads_as_a_waiver() {
        let source = |text: &str| {
            Tree::new()
                .tracked("crates/a/src/lib.rs")
                .file("crates/a/src/lib.rs", text)
        };
        let quiet = source(
            "// #[expect(clippy::all)]\n/* #[rustfmt::skip] */\n/// #[expect(unused)]\nconst A: &str = \"#[expect(unused)]\";\nconst B: &str = r#\"#[expect(warnings)]\"#;\nconst C: char = '#';\n#[expect(dead_code, reason = \"a fine reason\")]\n#[must_use]\nfn f() -> u8 { 0 }\n#[cfg_attr(test, tool::allow(clippy::all))]\nfn g() {}\n",
        );
        assert_eq!(
            quiet.run(PLAIN),
            Vec::<String>::new(),
            "a comment or a literal read as an attribute"
        );
        let raw = source(
            r##"const D: &str = r#"a"b #[expect(unused)] "#;
"##,
        );
        assert_eq!(
            raw.run(PLAIN),
            Vec::<String>::new(),
            "a raw string read as code at its inner quote"
        );
    }

    /// A waiver is refused in every spelling rustc and rustfmt read as one: a raw
    /// identifier, rustdoc's group, an attribute built from a macro argument, and
    /// source reached through `#[path]` or `include!`, which the scan never reads.
    #[test]
    fn a_waiver_in_another_spelling_is_refused() {
        let group = |line: usize, item: &str| {
            format!(
                "crates/a/src/lib.rs:{line} waives {item}, a lint group, and a waiver names the exact lint it waives"
            )
        };
        let skip = |line: usize| {
            format!(
                "crates/a/src/lib.rs:{line} carries rustfmt::skip, and rustfmt leaves the item unformatted with no reason given. Format it"
            )
        };
        let source = |text: &str| {
            Tree::new()
                .tracked("crates/a/src/lib.rs")
                .file("crates/a/src/lib.rs", text)
        };
        let cases = vec![
            (
                "a raw identifier",
                source("#[expect(r#unused, reason = \"x\")]\nfn f() {}\n"),
                group(1, "unused"),
            ),
            (
                "a raw rustfmt::skip",
                source("#[rustfmt::r#skip]\nfn f() {}\n"),
                skip(1),
            ),
            (
                "rustdoc's group",
                source("#![expect(rustdoc::all, reason = \"x\")]\n"),
                group(1, "rustdoc::all"),
            ),
            (
                "a waiver from a macro argument",
                source(
                    "macro_rules! w { ($l:ident) => { #[expect($l, reason = \"x\")] fn g() {} } }\n",
                ),
                "crates/a/src/lib.rs:1 is an attribute built from a macro argument, so what it waives is known only after expansion. Write the attribute out".to_string(),
            ),
            (
                "a module file by path",
                source("#[path = \"other.txt\"]\nmod m;\n"),
                "crates/a/src/lib.rs:1 sets a module's file with #[path], and the waiver scan reads tracked .rs files by name. Move the module to where its name puts it".to_string(),
            ),
            (
                "an include",
                source("fn f() {}\ninclude!(\"part.txt\");\n"),
                "crates/a/src/lib.rs:2 includes source with include!, and neither the waiver scan nor rustfmt reads the file it names. Make it a module".to_string(),
            ),
            (
                "an include by its path",
                source("fn f() {\n    let _ = [std::include!(\"part.txt\")];\n}\n"),
                "crates/a/src/lib.rs:2 includes source with include!, and neither the waiver scan nor rustfmt reads the file it names. Make it a module".to_string(),
            ),
        ];
        finds(cases, PLAIN);
        let found = source("fn f() {\n").run(PLAIN);
        assert_eq!(found.len(), 1, "a file that does not tokenize: {found:?}");
        assert!(
            found[0].starts_with("crates/a/src/lib.rs does not tokenize as Rust (")
                && found[0].ends_with("), so the lint waivers in it are unknown"),
            "{found:?}"
        );
        let other = source("#[cfg(test)]\nmod tests {}\nfn f() { include_str!(\"a\"); }\n");
        assert_eq!(
            other.run(PLAIN),
            Vec::<String>::new(),
            "include_str! reads no source"
        );
    }

    /// A reason no reader can see is blank however it is spelled: a zero-width
    /// space or a soft hyphen, raw or escaped, an escaped line break, or a line
    /// continuation. A raw string spells its backslashes as written.
    #[test]
    fn an_invisible_reason_is_blank() {
        let blank =
            "crates/a/src/lib.rs:1 gives a lint waiver a blank reason, and every waiver says why";
        let waiver = |reason: &str| {
            let text = format!("#[expect(dead_code, reason = \"{reason}\")]\nfn f() {{}}\n");
            Tree::new()
                .tracked("crates/a/src/lib.rs")
                .file("crates/a/src/lib.rs", &text)
        };
        let cases = vec![
            ("a zero-width space", waiver("\u{200B}"), blank.to_string()),
            ("a soft hyphen", waiver("\u{AD}"), blank.to_string()),
            (
                "an escaped zero-width space",
                waiver("\\u{200b}"),
                blank.to_string(),
            ),
            (
                "an escaped soft hyphen and a space",
                waiver("\\u{a_d} "),
                blank.to_string(),
            ),
            (
                "an escaped line break and tab",
                waiver("\\n\\t"),
                blank.to_string(),
            ),
            (
                "a byte escape for a space",
                waiver("\\x20"),
                blank.to_string(),
            ),
            ("a line continuation", waiver("\\\n    "), blank.to_string()),
            ("a braille blank", waiver("\u{2800}"), blank.to_string()),
        ];
        finds(cases, PLAIN);
        for (label, tree) in [
            ("a letter between invisible marks", waiver("a\u{200B}b")),
            ("an escaped letter", waiver("\\u{61}")),
            (
                "a raw string",
                Tree::new().tracked("crates/a/src/lib.rs").file(
                    "crates/a/src/lib.rs",
                    "#[expect(dead_code, reason = r\"\\u{200b}\")]\nfn f() {}\n",
                ),
            ),
        ] {
            assert_eq!(tree.run(PLAIN), Vec::<String>::new(), "{label}");
        }
    }

    /// An allow is refused on every platform, a cfg-gated one included, which
    /// clippy refuses only where its cfg holds, and an expect passes.
    #[test]
    fn an_allow_is_refused_on_every_platform() {
        let allow = |line: usize| {
            format!(
                "crates/a/src/lib.rs:{line} is an allow, and a waiver is an expect, which fails the build once its lint stops firing. clippy refuses an allow wherever its cfg holds, and this refuses one on every platform"
            )
        };
        let source = |text: &str| {
            Tree::new()
                .tracked("crates/a/src/lib.rs")
                .file("crates/a/src/lib.rs", text)
        };
        let cases = vec![
            (
                "an allow",
                source("#[allow(dead_code, reason = \"x\")]\nfn f() {}\n"),
                allow(1),
            ),
            (
                "an allow under a platform cfg",
                source(
                    "fn g() {}\n#[cfg_attr(not(windows), allow(dead_code, reason = \"x\"))]\nfn f() {}\n",
                ),
                allow(2),
            ),
        ];
        finds(cases, PLAIN);
        let gated =
            source("#[cfg_attr(not(windows), expect(dead_code, reason = \"x\"))]\nfn f() {}\n");
        assert_eq!(gated.run(PLAIN), Vec::<String>::new(), "a gated expect");
    }

    /// A waiver in an older or gated spelling is refused: rustfmt's old skip
    /// attribute, a module file set under `cfg_attr`, and `include` under
    /// another name.
    #[test]
    fn a_waiver_in_an_older_or_gated_spelling_is_refused() {
        let source = |text: &str| {
            Tree::new()
                .tracked("crates/a/src/lib.rs")
                .file("crates/a/src/lib.rs", text)
        };
        let imports = "crates/a/src/lib.rs:1 imports include, and under any name it reads a file neither the waiver scan nor rustfmt reads. Make it a module";
        let cases = vec![
            (
                "rustfmt's old skip attribute",
                source("#[cfg_attr(rustfmt, rustfmt_skip)]\nfn f() {}\n"),
                "crates/a/src/lib.rs:1 carries rustfmt::skip, and rustfmt leaves the item unformatted with no reason given. Format it".to_string(),
            ),
            (
                "a module file under cfg_attr",
                source("#[cfg_attr(all(), path = \"other.txt\")]\nmod other;\n"),
                "crates/a/src/lib.rs:1 sets a module's file with #[path], and the waiver scan reads tracked .rs files by name. Move the module to where its name puts it".to_string(),
            ),
            (
                "include imported under another name",
                source("use core::include as inc;\n"),
                imports.to_string(),
            ),
            (
                "include imported in a group",
                source("use core::{include as inc, line};\n"),
                imports.to_string(),
            ),
        ];
        finds(cases, PLAIN);
        let other = source("use core::{include_str, line};\n");
        assert_eq!(
            other.run(PLAIN),
            Vec::<String>::new(),
            "an import of include_str"
        );
    }

    /// Every crate takes the workspace's lints and nothing of its own, and the
    /// root denies a waiver that is an allow or has no reason.
    #[test]
    fn every_crate_takes_the_workspace_lints() {
        let own = |path: &str| {
            format!(
                "{path:?} does not hold [lints] workspace = true alone, and a crate that sets its own lints can turn off what the workspace forbids"
            )
        };
        let manifest = |text: &str| {
            Tree::new()
                .tracked("crates/a/Cargo.toml")
                .file("crates/a/Cargo.toml", text)
        };
        let cases = vec![
            ("no lints table", manifest("[package]\nname = \"a\"\n"), own("crates/a/Cargo.toml")),
            ("lints of its own", manifest("[package]\nname = \"a\"\n\n[lints.clippy]\nall = \"allow\"\n"), own("crates/a/Cargo.toml")),
            ("the workspace's and its own", manifest("[package]\nname = \"a\"\n\n[lints]\nworkspace = true\n\n[lints.rust]\nunsafe_code = \"allow\"\n"), own("crates/a/Cargo.toml")),
            ("the workspace's turned off", manifest("[package]\nname = \"a\"\n\n[lints]\nworkspace = false\n"), own("crates/a/Cargo.toml")),
            (
                "the reason lint at forbid",
                Tree::new().file("Cargo.toml", "[workspace.lints.clippy]\nallow_attributes = \"deny\"\nallow_attributes_without_reason = \"forbid\"\n"),
                "Cargo.toml does not set allow_attributes_without_reason = \"deny\" under [workspace.lints.clippy], so a lint waiver can go without a reason".to_string(),
            ),
            (
                "the allow lint at warn",
                Tree::new().file("Cargo.toml", "[workspace.lints.clippy]\nallow_attributes = \"warn\"\nallow_attributes_without_reason = \"deny\"\n"),
                "Cargo.toml does not set allow_attributes = \"deny\" under [workspace.lints.clippy], so a waiver can be an allow, which stays silent once its lint stops firing".to_string(),
            ),
        ];
        finds(cases, PLAIN);
        let silent = Tree::new().file("Cargo.toml", "[workspace]\nmembers = []\n");
        assert_eq!(
            silent.run(PLAIN),
            vec![
                "Cargo.toml does not set allow_attributes = \"deny\" under [workspace.lints.clippy], so a waiver can be an allow, which stays silent once its lint stops firing".to_string(),
                "Cargo.toml does not set allow_attributes_without_reason = \"deny\" under [workspace.lints.clippy], so a lint waiver can go without a reason".to_string(),
            ],
            "the root silent"
        );
        let inherited = manifest("[package]\nname = \"a\"\n\n[lints]\nworkspace = true\n");
        assert_eq!(inherited.run(PLAIN), Vec::<String>::new());
    }

    /// A cargo-machete ignore list is refused in any manifest, a member's or the
    /// root's.
    #[test]
    fn a_cargo_machete_ignore_list_is_refused() {
        let refused = |path: &str, section: &str| {
            format!(
                "{path:?} carries [{section}.metadata.cargo-machete], and cargo-machete passes every dependency it lists unread. Remove the dependency until code uses it"
            )
        };
        let cases = vec![
            (
                "a member's list",
                Tree::new().tracked("crates/a/Cargo.toml").file(
                    "crates/a/Cargo.toml",
                    "[package]\nname = \"a\"\n\n[lints]\nworkspace = true\n\n[package.metadata.cargo-machete]\nignored = [\"serde\"]\n",
                ),
                refused("crates/a/Cargo.toml", "package"),
            ),
            (
                "the root's list",
                Tree::new().tracked("Cargo.toml").file(
                    "Cargo.toml",
                    "[workspace.lints.clippy]\nallow_attributes = \"deny\"\nallow_attributes_without_reason = \"deny\"\n\n[workspace.metadata.cargo-machete]\nignored = [\"serde\"]\n",
                ),
                refused("Cargo.toml", "workspace"),
            ),
        ];
        finds(cases, PLAIN);
    }

    /// JSON the gate parses refuses a repeated key at any depth, and an escaped
    /// spelling counts as the key it spells.
    #[test]
    fn a_repeated_json_key_is_refused_at_any_depth() {
        assert!(parse_json(r#"{ "a": { "b": 1, "c": [ { "d": 1 } ] } }"#).is_ok());
        for text in [
            r#"{ "a": 1, "a": 2 }"#,
            r#"{ "x": { "b": 1, "b": 2 } }"#,
            r#"{ "x": [ { "d": 1, "d": 2 } ] }"#,
            r#"{ "a": 1, "\u0061": 2 }"#,
        ] {
            let why = parse_json(text).expect_err(text);
            assert!(why.starts_with("it repeats "), "{text}: {why}");
        }
        assert!(parse_json(r#"{ "a": "\"a\": 1" , "b": ["a", "a"] }"#).is_ok());
    }
}
