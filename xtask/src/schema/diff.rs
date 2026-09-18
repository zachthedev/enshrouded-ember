//! The delta between two extractions.
//!
//! The point of this command is one hour, not one afternoon. Enshrouded 1.0
//! ships on 2026-10-15, and a native container permission feature in that build
//! changes what this project should build. Every name carrying one of the
//! [`WATCHLIST`] words is reported before anything else, so the answer is the
//! first thing on the screen.
//!
//! Addresses are deliberately not compared. A descriptor's base moves with
//! every relink, so a diff on addresses is all noise. What is compared is
//! shape: sizes, alignments, flags, hashes, field offsets and field types.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::root::DevRoot;
use crate::ui::{Mark, Row, Ui};

/// Names worth seeing first, matched case insensitively as substrings.
pub const WATCHLIST: [&str; 11] = [
    "container",
    "inventory",
    "chest",
    "scope",
    "permission",
    "owner",
    "lock",
    "group",
    "team",
    "station",
    "base",
];

/// One member of a type, which is a field or an enumerator.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Member {
    /// The byte offset of a field, or the value of an enumerator.
    key: String,
    /// The member name.
    name: String,
    /// The member's type, which an enumerator has none of.
    type_name: String,
}

/// The shape of one type, with everything that moves between relinks removed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct TypeShape {
    /// Size of the type as the descriptor states it.
    size: String,
    /// Alignment of the type.
    align: String,
    /// Reflection flags.
    flags: String,
    /// The 64-bit hash.
    hash: String,
    /// Fields, or enumerators for an enumeration.
    members: Vec<Member>,
}

/// One extraction, parsed into what a diff compares.
struct Extraction {
    /// Every `keen::` type, by qualified name.
    types: BTreeMap<String, TypeShape>,
    /// Every program name, with how many records carry it.
    programs: BTreeMap<String, usize>,
    /// Every network message name.
    messages: BTreeSet<String>,
    /// Every UI surface name.
    ui_events: BTreeSet<String>,
}

/// One reported difference.
struct Change {
    mark: Mark,
    name: String,
    what: String,
    detail: String,
}

impl Change {
    /// Whether this change touches a watchlist name.
    fn on_watchlist(&self) -> bool {
        is_watched(&self.name) || is_watched(&self.what)
    }

    /// Render as a result row.
    fn row(&self) -> Row {
        Row::new(self.mark, self.name.as_str(), self.what.as_str()).note(self.detail.as_str())
    }
}

/// Whether a name carries a watchlist word.
pub fn is_watched(name: &str) -> bool {
    let lowered = name.to_ascii_lowercase();
    WATCHLIST.iter().any(|word| lowered.contains(word))
}

/// Compare two extractions and print what moved.
///
/// # Errors
///
/// Returns an error when either extraction cannot be found or read.
pub fn run(root: &DevRoot, ui: &Ui, old: &str, new: &str) -> Result<bool> {
    let dir = root.schema_dir();
    let old_dir = super::resolve_extraction(&dir, old)?;
    let new_dir = super::resolve_extraction(&dir, new)?;
    let before = load(&old_dir)?;
    let after = load(&new_dir)?;

    ui.section("schema diff");
    ui.line(&format!("{} to {}", describe(&old_dir), describe(&new_dir)));

    let differences = compare(&before, &after);
    let watched: Vec<&Change> = differences.iter().filter(|c| c.on_watchlist()).collect();

    ui.section("watchlist");
    if watched.is_empty() {
        ui.line("no name on the watchlist changed");
    } else {
        let rows: Vec<Row> = watched.iter().map(|change| change.row()).collect();
        ui.rows(&rows);
    }

    for (label, prefix) in [
        ("types", "type"),
        ("programs", "program"),
        ("messages", "message"),
        ("ui events", "ui event"),
    ] {
        let rows: Vec<Row> = differences
            .iter()
            .filter(|change| change.what.starts_with(prefix))
            .map(Change::row)
            .collect();
        ui.section(label);
        if rows.is_empty() {
            ui.line("unchanged");
        } else {
            ui.rows(&rows);
        }
    }

    ui.section("summary");
    let added = count(&differences, Mark::Added);
    let removed = count(&differences, Mark::Removed);
    let changed = count(&differences, Mark::Changed);
    ui.rows(&[
        Row::new(Mark::Note, "added", added.to_string()),
        Row::new(Mark::Note, "removed", removed.to_string()),
        Row::new(Mark::Note, "changed", changed.to_string()),
        Row::new(Mark::Note, "on the watchlist", watched.len().to_string()),
    ]);
    ui.divider(34);
    ui.summary(
        &format!("{} differences", differences.len()),
        !watched.is_empty(),
    );
    Ok(true)
}

/// How many changes carry a mark.
fn count(changes: &[Change], mark: Mark) -> usize {
    changes.iter().filter(|change| change.mark == mark).count()
}

/// How one extraction names itself in the heading.
///
/// The record beside the dumps carries the revision and the extraction time, so
/// a diff says which two builds it compared without anyone opening an image. An
/// extraction with no record falls back to its directory name.
fn describe(path: &Path) -> String {
    match super::metadata::BuildRecord::read(path) {
        Ok(Some(record)) => record.summary(),
        _ => name_of(path),
    }
}

/// The directory name, which is the buildid.
fn name_of(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// Read one extraction directory.
///
/// Every file read here is written by every `schema extract` run, whatever flags
/// it was given. One that is absent means the extraction stopped partway, so it
/// is refused by name. Parsing it as an empty section instead would report every
/// type, program, message and surface in it as removed, which is a wrong answer
/// rather than a missing one.
fn load(dir: &Path) -> Result<Extraction> {
    Ok(Extraction {
        types: parse_schema(&require(dir, "srv.schema.txt")?),
        programs: parse_programs(&require(dir, "programs.tsv")?),
        messages: parse_messages(&require(dir, "srv.proto.txt")?),
        ui_events: parse_ui_events(&require(dir, "ui-events.txt")?),
    })
}

/// Read a file the extraction has to hold.
fn require(dir: &Path, name: &str) -> Result<String> {
    let path = dir.join(name);
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => bail!(
            "{} holds no {name}, so that extraction did not finish. Take it again with `cargo xtask schema extract --force`.",
            dir.display()
        ),
        Err(err) => Err(err).with_context(|| format!("reading {}", path.display())),
    }
}

/// Parse a schema dump into type shapes.
fn parse_schema(text: &str) -> BTreeMap<String, TypeShape> {
    let mut out: BTreeMap<String, TypeShape> = BTreeMap::new();
    let mut current: Option<(String, TypeShape)> = None;
    for line in text.lines() {
        if line.is_empty() {
            if let Some((name, shape)) = current.take() {
                out.insert(name, shape);
            }
            continue;
        }
        if let Some(field) = parse_field(line) {
            if let Some((_, shape)) = current.as_mut() {
                shape.members.push(field);
            }
            continue;
        }
        if let Some(value) = parse_enumerator(line) {
            if let Some((_, shape)) = current.as_mut() {
                shape.members.push(value);
            }
            continue;
        }
        if let Some((name, shape)) = parse_header(line)
            && let Some((previous, built)) = current.replace((name, shape))
        {
            out.insert(previous, built);
        }
    }
    if let Some((name, shape)) = current.take() {
        out.insert(name, shape);
    }
    out
}

/// Parse a type header line.
fn parse_header(line: &str) -> Option<(String, TypeShape)> {
    let (name, rest) = line.split_once("  @")?;
    let mut shape = TypeShape::default();
    for token in rest.split_whitespace() {
        let Some((key, value)) = token.split_once('=') else {
            continue;
        };
        match key {
            "size" => shape.size = value.to_string(),
            "align" => shape.align = value.to_string(),
            "flags" => shape.flags = value.to_string(),
            "hash" => shape.hash = value.to_string(),
            _ => {}
        }
    }
    Some((name.to_string(), shape))
}

/// Parse a field line.
fn parse_field(line: &str) -> Option<Member> {
    let rest = line.strip_prefix("    +")?;
    let (offset, rest) = rest.split_once("  ")?;
    let (name, rest) = rest.trim_start().split_once(' ')?;
    let rest = rest.trim_start().strip_prefix(": ")?;
    let (type_name, _) = rest.rsplit_once("  [")?;
    Some(Member {
        key: offset.to_string(),
        name: name.to_string(),
        type_name: type_name.to_string(),
    })
}

/// Parse an enumerator line.
fn parse_enumerator(line: &str) -> Option<Member> {
    let rest = line.strip_prefix("    =")?;
    let (value, name) = rest.split_once(' ')?;
    Some(Member {
        key: value.to_string(),
        name: name.trim_start().to_string(),
        type_name: String::new(),
    })
}

/// Parse a program table into name counts.
fn parse_programs(text: &str) -> BTreeMap<String, usize> {
    let mut out: BTreeMap<String, usize> = BTreeMap::new();
    for line in text.lines().skip(1) {
        let mut columns = line.split('\t');
        let (Some(_), Some(name)) = (columns.next(), columns.next()) else {
            continue;
        };
        if !name.is_empty() {
            *out.entry(name.to_string()).or_default() += 1;
        }
    }
    out
}

/// Parse a message registry dump into message names.
fn parse_messages(text: &str) -> BTreeSet<String> {
    text.lines()
        .filter(|line| !line.is_empty() && !line.starts_with(' ') && !line.starts_with('['))
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_string)
        .collect()
}

/// Parse a UI surface dump into names.
fn parse_ui_events(text: &str) -> BTreeSet<String> {
    text.lines()
        .filter(|line| !line.is_empty() && !line.starts_with(' '))
        .map(str::to_string)
        .collect()
}

/// Every difference between two extractions.
fn compare(before: &Extraction, after: &Extraction) -> Vec<Change> {
    let mut out = Vec::new();
    compare_types(before, after, &mut out);
    compare_counts(&before.programs, &after.programs, "program", &mut out);
    compare_sets(&before.messages, &after.messages, "message", &mut out);
    compare_sets(&before.ui_events, &after.ui_events, "ui event", &mut out);
    out
}

/// Added, removed and changed types, with per-member deltas.
fn compare_types(before: &Extraction, after: &Extraction, out: &mut Vec<Change>) {
    for name in after.types.keys() {
        if !before.types.contains_key(name) {
            out.push(Change {
                mark: Mark::Added,
                name: name.clone(),
                what: "type added".to_string(),
                detail: String::new(),
            });
        }
    }
    for name in before.types.keys() {
        if !after.types.contains_key(name) {
            out.push(Change {
                mark: Mark::Removed,
                name: name.clone(),
                what: "type removed".to_string(),
                detail: String::new(),
            });
        }
    }
    for (name, old) in &before.types {
        let Some(new) = after.types.get(name) else {
            continue;
        };
        if old == new {
            continue;
        }
        for (label, was, now) in [
            ("type size", &old.size, &new.size),
            ("type align", &old.align, &new.align),
            ("type flags", &old.flags, &new.flags),
            ("type hash", &old.hash, &new.hash),
        ] {
            if was != now {
                out.push(Change {
                    mark: Mark::Changed,
                    name: name.clone(),
                    what: label.to_string(),
                    detail: format!("{was} to {now}"),
                });
            }
        }
        compare_members(name, &old.members, &new.members, out);
    }
}

/// Per-member deltas inside one type.
fn compare_members(type_name: &str, old: &[Member], new: &[Member], out: &mut Vec<Change>) {
    let old_by_name: BTreeMap<&str, &Member> = old.iter().map(|m| (m.name.as_str(), m)).collect();
    let new_by_name: BTreeMap<&str, &Member> = new.iter().map(|m| (m.name.as_str(), m)).collect();
    for (member, entry) in &new_by_name {
        if !old_by_name.contains_key(member) {
            out.push(Change {
                mark: Mark::Added,
                name: type_name.to_string(),
                what: format!("type member {member} added"),
                detail: format!("at {}", entry.key),
            });
        }
    }
    for (member, entry) in &old_by_name {
        if !new_by_name.contains_key(member) {
            out.push(Change {
                mark: Mark::Removed,
                name: type_name.to_string(),
                what: format!("type member {member} removed"),
                detail: format!("was at {}", entry.key),
            });
        }
    }
    for (member, was) in &old_by_name {
        let Some(now) = new_by_name.get(member) else {
            continue;
        };
        if was.key != now.key {
            out.push(Change {
                mark: Mark::Changed,
                name: type_name.to_string(),
                what: format!("type member {member} moved"),
                detail: format!("{} to {}", was.key, now.key),
            });
        }
        if was.type_name != now.type_name {
            out.push(Change {
                mark: Mark::Changed,
                name: type_name.to_string(),
                what: format!("type member {member} retyped"),
                detail: format!("{} to {}", was.type_name, now.type_name),
            });
        }
    }
}

/// Added and removed names in a counted collection.
fn compare_counts(
    before: &BTreeMap<String, usize>,
    after: &BTreeMap<String, usize>,
    label: &str,
    out: &mut Vec<Change>,
) {
    for (name, count) in after {
        let was = before.get(name).copied().unwrap_or(0);
        if *count > was {
            out.push(Change {
                mark: Mark::Added,
                name: name.clone(),
                what: format!("{label} added"),
                detail: if was == 0 {
                    String::new()
                } else {
                    format!("{was} to {count} records")
                },
            });
        }
    }
    for (name, count) in before {
        let now = after.get(name).copied().unwrap_or(0);
        if *count > now {
            out.push(Change {
                mark: Mark::Removed,
                name: name.clone(),
                what: format!("{label} removed"),
                detail: if now == 0 {
                    String::new()
                } else {
                    format!("{count} to {now} records")
                },
            });
        }
    }
}

/// Added and removed names in a set.
fn compare_sets(
    before: &BTreeSet<String>,
    after: &BTreeSet<String>,
    label: &str,
    out: &mut Vec<Change>,
) {
    for name in after.difference(before) {
        out.push(Change {
            mark: Mark::Added,
            name: name.clone(),
            what: format!("{label} added"),
            detail: String::new(),
        });
    }
    for name in before.difference(after) {
        out.push(Change {
            mark: Mark::Removed,
            name: name.clone(),
            what: format!("{label} removed"),
            detail: String::new(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{Extraction, Mark, WATCHLIST, compare, is_watched, load, parse_schema};
    use crate::testutil::TestDir;
    use std::collections::{BTreeMap, BTreeSet};

    /// The four files `load` reads, which every extract run writes.
    const REQUIRED: [&str; 4] = [
        "srv.schema.txt",
        "programs.tsv",
        "srv.proto.txt",
        "ui-events.txt",
    ];

    /// A run that stops partway leaves some of the four behind. Reading the rest
    /// as empty sections would report every type, program, message and surface
    /// in the missing file as removed, which is a wrong answer rather than a
    /// missing one.
    #[test]
    fn an_extraction_missing_a_file_the_diff_reads_is_refused_by_name() {
        for absent in REQUIRED {
            let dir = TestDir::new("diff-short");
            for name in REQUIRED {
                if name != absent {
                    dir.write(&format!("23178631/{name}"), b"");
                }
            }
            let target = dir.path().join("23178631");

            let outcome = load(&target);

            assert!(
                outcome.is_err(),
                "an extraction with no {absent} must be refused, not read as empty"
            );
            let text = outcome
                .err()
                .map_or_else(String::new, |err| format!("{err:#}"));
            assert!(
                text.contains(absent),
                "the refusal names the missing file: {text}"
            );
        }
    }

    /// An extraction holding all four reads, whatever else it does or does not
    /// carry. `cli.schema.txt`, `descriptors.tsv` and `strings.tsv` are absent
    /// here, and none of them is read by a diff.
    #[test]
    fn an_extraction_holding_the_four_reads_without_the_files_no_diff_opens() {
        let dir = TestDir::new("diff-complete");
        for name in REQUIRED {
            dir.write(&format!("23178631/{name}"), b"");
        }

        load(&dir.path().join("23178631")).expect("the four the diff reads are all there");
    }

    /// Build a schema dump body from header and member lines.
    fn schema(blocks: &[(&str, &str, &[&str])]) -> String {
        use std::fmt::Write as _;

        let mut out = String::new();
        for (name, header, members) in blocks {
            let _ = writeln!(out, "{name}  @0x140000000 {header}");
            for member in *members {
                out.push_str(member);
                out.push('\n');
            }
            out.push('\n');
        }
        out
    }

    /// A field line in the dump's exact column layout.
    fn field(offset: &str, name: &str, type_name: &str, size: &str) -> String {
        format!("    +{offset}  {name:<44} : {type_name}  [{size}]")
    }

    fn extraction(types: &str, programs: &[&str], messages: &[&str], ui: &[&str]) -> Extraction {
        let mut program_counts: BTreeMap<String, usize> = BTreeMap::new();
        for name in programs {
            *program_counts.entry((*name).to_string()).or_default() += 1;
        }
        Extraction {
            types: parse_schema(types),
            programs: program_counts,
            messages: messages.iter().map(|m| (*m).to_string()).collect(),
            ui_events: ui.iter().map(|m| (*m).to_string()).collect::<BTreeSet<_>>(),
        }
    }

    #[test]
    fn a_field_line_round_trips_through_the_parser() {
        let dump = schema(&[(
            "keen::ecs::Inventory",
            "size=0x20 align=4 nfields=1 flags=0x12 hash=0x0000000000000001",
            &[&field("0x0010", "slotCount", "keen::uint32", "4")],
        )]);
        let parsed = parse_schema(&dump);
        let shape = parsed.get("keen::ecs::Inventory").expect("the type parses");
        assert_eq!(shape.size, "0x20");
        assert_eq!(shape.align, "4");
        assert_eq!(shape.members.len(), 1);
        assert_eq!(shape.members[0].name, "slotCount");
        assert_eq!(shape.members[0].key, "0x0010");
        assert_eq!(shape.members[0].type_name, "keen::uint32");
    }

    #[test]
    fn every_kind_of_difference_is_reported() {
        let header = "size=0x20 align=4 nfields=1 flags=0x12 hash=0x0000000000000001";
        let before = extraction(
            &schema(&[
                (
                    "keen::ecs::Inventory",
                    header,
                    &[&field("0x0010", "slotCount", "keen::uint32", "4")],
                ),
                ("keen::ecs::Retired", header, &[]),
            ]),
            &["player_crafting", "cache_building_stock"],
            &["ChatMessageServerMessage", "RetiredMessage"],
            &["keen::ecs::UiWorldEvent", "keen::ecs::UiRetiredEvent"],
        );
        let after = extraction(
            &schema(&[
                (
                    "keen::ecs::Inventory",
                    header,
                    &[&field("0x0014", "slotCount", "keen::uint64", "8")],
                ),
                ("keen::ecs::ContainerScope", header, &[]),
            ]),
            &["player_crafting", "container_permission"],
            &["ChatMessageServerMessage", "ContainerLockMessage"],
            &["keen::ecs::UiWorldEvent", "keen::ecs::UiChestScopeEvent"],
        );

        let changes = compare(&before, &after);
        let found: Vec<(Mark, &str, &str, &str)> = changes
            .iter()
            .map(|c| (c.mark, c.name.as_str(), c.what.as_str(), c.detail.as_str()))
            .collect();

        let want: &[(Mark, &str, &str)] = &[
            (Mark::Added, "keen::ecs::ContainerScope", "type added"),
            (Mark::Removed, "keen::ecs::Retired", "type removed"),
            (
                Mark::Changed,
                "keen::ecs::Inventory",
                "type member slotCount moved",
            ),
            (
                Mark::Changed,
                "keen::ecs::Inventory",
                "type member slotCount retyped",
            ),
            (Mark::Added, "container_permission", "program added"),
            (Mark::Removed, "cache_building_stock", "program removed"),
            (Mark::Added, "ContainerLockMessage", "message added"),
            (Mark::Removed, "RetiredMessage", "message removed"),
            (
                Mark::Added,
                "keen::ecs::UiChestScopeEvent",
                "ui event added",
            ),
            (
                Mark::Removed,
                "keen::ecs::UiRetiredEvent",
                "ui event removed",
            ),
        ];
        for (mark, name, what) in want {
            assert!(
                found
                    .iter()
                    .any(|(m, n, w, _)| m == mark && n == name && w == what),
                "missing {what} for {name}; got {found:#?}"
            );
        }

        let moved = found
            .iter()
            .find(|(_, _, what, _)| *what == "type member slotCount moved")
            .expect("the offset delta is reported");
        assert_eq!(moved.3, "0x0010 to 0x0014");
        let retyped = found
            .iter()
            .find(|(_, _, what, _)| *what == "type member slotCount retyped")
            .expect("the type delta is reported");
        assert_eq!(retyped.3, "keen::uint32 to keen::uint64");
    }

    #[test]
    fn every_watchlist_word_is_matched_case_insensitively() {
        for word in WATCHLIST {
            let name = format!("keen::ecs::Prefix{}Suffix", capitalize(word));
            assert!(is_watched(&name), "watchlist word {word} must match {name}");
            assert!(
                is_watched(&name.to_uppercase()),
                "watchlist word {word} must match in upper case"
            );
        }
        assert!(!is_watched("keen::ecs::CurrentTransform"));
        assert!(!is_watched("keen::ecs::Velocity"));
    }

    /// Upper-case the first byte, which is enough for the ASCII words here.
    fn capitalize(word: &str) -> String {
        let mut chars = word.chars();
        match chars.next() {
            Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
            None => String::new(),
        }
    }
}
