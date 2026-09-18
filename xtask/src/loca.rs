//! Materialization of a client build's localization tables.
//!
//! A mod that displays a line names a `LocaTagId`, and only the client carries
//! the table that resolves one. `ember_kfc::loca` states what a caller may keep
//! from a decoded table. This command is the build step that rule describes, so
//! everything it writes lands under the gitignored `.cache/loca/<buildid>` and
//! is regenerated from a client the developer fetched.
//!
//! One tab-separated file per language, because a developer reads the text to
//! find the id and commits the id alone.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use ember_kfc::loca::{ENGLISH, KEENGLISH, LocaTag, TagCollection, decode_tags};
use ember_kfc::{Archive, ContentHash};

use crate::LocaCommand;
use crate::root::{self, DevRoot};
use crate::ui::{Mark, Row, Ui};

/// The columns every table opens with.
///
/// The id is decimal, because that is the spelling a mod's own constant takes.
const HEADER: &str = "id\targuments\tgeneric\ttext";

/// The line ending every table uses, matching the schema dumps.
const EOL: &str = "\r\n";

/// Run one `loca` subcommand.
///
/// # Errors
///
/// Returns an error when the client or its container set cannot be read, when
/// the buildid cannot be resolved, or when an output cannot be written.
pub fn run(command: &LocaCommand, root: &DevRoot, ui: &Ui) -> Result<bool> {
    match command {
        LocaCommand::Extract {
            client,
            build,
            out,
            language,
            force,
        } => extract(
            root,
            ui,
            client,
            build.as_deref(),
            out.as_deref(),
            language,
            *force,
        ),
    }
}

/// Decode one client's tables and write them out.
fn extract(
    root: &DevRoot,
    ui: &Ui,
    client: &Path,
    build: Option<&str>,
    out: Option<&Path>,
    wanted: &[u32],
    force: bool,
) -> Result<bool> {
    require_container_set(client)?;
    let build_id = locate_build(client, build)?;
    let target = target_dir(root, &build_id, out)?;
    if target.exists() && !force {
        bail!(
            "{} already holds an extraction of build {build_id}. Pass --force to replace it.",
            target.display()
        );
    }

    ui.section("loca extract");
    ui.line(&format!("build {build_id} from {}", client.display()));

    let mut archive = Archive::beside(client)
        .with_context(|| format!("reading the container set beside {}", client.display()))?;
    let collection = read_collection(&mut archive, client)?;
    let selected = select_languages(&collection, wanted)?;

    let cleared = prepare_target(&target)?;
    if cleared > 0 {
        ui.line(&format!("replaced {cleared} table(s) already there"));
    }

    let mut rows = Vec::with_capacity(selected.len());
    for (id, hash) in selected {
        let blob = archive
            .read_content(hash)
            .with_context(|| format!("reading language {id}'s table"))?;
        let tags = decode_tags(&blob).with_context(|| format!("decoding language {id}'s table"))?;
        rows.push(write_table(&target, id, &tags)?);
    }

    ui.rows(&rows);
    ui.line(&format!("written to {}", target.display()));
    Ok(true)
}

/// Refuse a client whose container set is not beside it.
///
/// An archived build carries the executable and the `.kfc` directory alone, so
/// the payload is the file that is missing and the one worth naming. Opening the
/// archive would report the same absence as a read error several frames down.
fn require_container_set(client: &Path) -> Result<()> {
    if !client.is_file() {
        bail!(
            "no client executable at {}. Pass --client <exe> naming an installed Enshrouded client.",
            client.display()
        );
    }
    let directory = client.with_extension("kfc");
    if !directory.is_file() {
        bail!(
            "no container directory at {}. A localization table is read from the container set beside the executable.",
            directory.display()
        );
    }
    let payload = client.with_extension("kfc_resources");
    if !payload.is_file() {
        bail!(
            "no payload at {}. An archived build carries the executable and {} alone, so its resources cannot be read. Point --client at a full client install.",
            payload.display(),
            directory.display()
        );
    }
    Ok(())
}

/// The buildid naming the output directory.
///
/// A typed `--build` wins. Otherwise the app manifest names it, and both
/// layouts that put one on disk are searched: `SteamCMD` writes `steamapps`
/// inside the install directory it was given, while a Steam library keeps one
/// `steamapps` beside the whole `common` tree its games sit in.
fn locate_build(client: &Path, build: Option<&str>) -> Result<String> {
    if let Some(named) = build {
        root::check_build_id(named)?;
        return Ok(named.to_string());
    }
    let install = client.parent().unwrap_or(Path::new("."));
    let library = install
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent);
    let mut tried: Vec<PathBuf> = vec![install.to_path_buf()];
    if let Ok(found) = root::build_id_from_manifest(install, root::CLIENT_APP_ID) {
        return Ok(found);
    }
    if let Some(library) = library {
        tried.push(library.to_path_buf());
        if let Ok(found) = root::build_id_from_manifest(library, root::CLIENT_APP_ID) {
            return Ok(found);
        }
    }
    let names: Vec<String> = tried
        .iter()
        .map(|path| path.display().to_string())
        .collect();
    bail!(
        "no app manifest under a steamapps directory in {}. Pass --build <buildid> to name the extraction directory for {}.",
        names.join(" or "),
        client.display()
    )
}

/// Where the tables land.
///
/// An explicit `--out` is held to the Steam refusal like every directory this
/// repository writes into, because a path a caller types reaches the filesystem
/// the same way one derived from a buildid does.
fn target_dir(root: &DevRoot, build_id: &str, out: Option<&Path>) -> Result<PathBuf> {
    let Some(path) = out else {
        return root.loca_build_dir(build_id);
    };
    crate::steam::ensure_outside_library(path, "a localization directory")?;
    Ok(path.to_path_buf())
}

/// Make the target directory hold this run's tables and nothing else, and
/// report how many it removed.
///
/// A narrowed run writes fewer files than a full one, so leaving the previous
/// run's tables in place would present them as part of this extraction. Only
/// the extension this command writes is removed, because `--out` names a
/// directory the caller chose and whatever else is in it is theirs.
fn prepare_target(target: &Path) -> Result<usize> {
    std::fs::create_dir_all(target).with_context(|| format!("creating {}", target.display()))?;
    let entries =
        std::fs::read_dir(target).with_context(|| format!("reading {}", target.display()))?;
    let stale: Vec<PathBuf> = entries
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "tsv"))
        .collect();
    for path in &stale {
        std::fs::remove_file(path).with_context(|| format!("removing {}", path.display()))?;
    }
    Ok(stale.len())
}

/// Read and parse the one `keen::LocaTagCollectionResource` a client ships.
fn read_collection(archive: &mut Archive, client: &Path) -> Result<TagCollection> {
    let mut found = archive.directory().by_type(TagCollection::TYPE_HASH);
    let Some(resource) = found.next().copied() else {
        bail!(
            "{} ships no {}. A dedicated server carries none, so a client build is the only source of one.",
            client.display(),
            TagCollection::TYPE_NAME
        );
    };
    let extra = found.count();
    if extra > 0 {
        bail!(
            "{} ships {} localization tables, and the format allows one.",
            client.display(),
            extra + 1
        );
    }
    let bytes = archive
        .read_resource(resource.id)
        .with_context(|| format!("reading {} out of the payload", TagCollection::TYPE_NAME))?;
    TagCollection::parse(&bytes).with_context(|| format!("parsing {}", TagCollection::TYPE_NAME))
}

/// The languages to write, as `(id, hash)` in ascending id order.
///
/// With none named, every table the collection carries is written, authoring
/// language first. A named id that the build does not ship is refused by name
/// rather than skipped, because a silent skip leaves an empty directory looking
/// like a successful extraction.
fn select_languages(collection: &TagCollection, wanted: &[u32]) -> Result<Vec<(u32, ContentHash)>> {
    if wanted.is_empty() {
        let mut all = vec![(KEENGLISH, collection.keenglish)];
        let mut shipped = collection.languages.clone();
        shipped.sort_unstable_by_key(|(id, _)| *id);
        all.extend(shipped);
        return Ok(all);
    }
    let mut ids: Vec<u32> = wanted.to_vec();
    ids.sort_unstable();
    ids.dedup();
    ids.iter()
        .map(|id| {
            collection
                .language(*id)
                .map(|hash| (*id, hash))
                .ok_or_else(|| {
                    let shipped: Vec<String> = std::iter::once(KEENGLISH)
                        .chain(collection.languages.iter().map(|(id, _)| *id))
                        .map(|id| id.to_string())
                        .collect();
                    anyhow::anyhow!(
                        "this build ships no language {id}. It carries {}.",
                        shipped.join(", ")
                    )
                })
        })
        .collect()
}

/// The file one language's table is written to.
///
/// The crate names two ids and nothing here invents a name for the rest, so an
/// unidentified language keeps its number.
fn table_name(id: u32) -> String {
    match id {
        KEENGLISH => "keenglish.tsv".to_string(),
        ENGLISH => "english.tsv".to_string(),
        other => format!("language-{other}.tsv"),
    }
}

/// Write one language's table and describe it as a result row.
fn write_table(target: &Path, id: u32, tags: &[LocaTag]) -> Result<Row> {
    let name = table_name(id);
    let body = table(tags);
    let path = target.join(&name);
    std::fs::write(&path, &body).with_context(|| format!("writing {}", path.display()))?;
    Ok(Row::new(Mark::Ok, name, tags.len().to_string()).note(crate::ui::bytes(body.len() as u64)))
}

/// One language's table, as tab-separated rows under a header.
fn table(tags: &[LocaTag]) -> String {
    let mut out = String::with_capacity(tags.len() * 64);
    let _ = write!(out, "{HEADER}{EOL}");
    for tag in tags {
        let _ = write!(
            out,
            "{}\t{}\t{}\t{}{EOL}",
            tag.id,
            tag.arguments,
            tag.generic_arguments,
            escape(&tag.text)
        );
    }
    out
}

/// One tag's text, with everything that would end a row or a column escaped.
///
/// UI text carries line breaks, so a row holding a raw one would read as two
/// rows and shift every column after it.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        EOL, escape, locate_build, prepare_target, require_container_set, select_languages, table,
        table_name,
    };
    use crate::testutil::TestDir;
    use ember_kfc::ContentHash;
    use ember_kfc::loca::{LocaTag, TagCollection};

    fn tag(id: u32, text: &str) -> LocaTag {
        LocaTag {
            id,
            text: text.to_string(),
            text_at: 0,
            arguments: 0,
            generic_arguments: 0,
        }
    }

    fn collection() -> TagCollection {
        TagCollection {
            keenglish: ContentHash::new(10, [0, 0, 0]),
            languages: vec![
                (2, ContentHash::new(30, [0, 0, 0])),
                (1, ContentHash::new(20, [0, 0, 0])),
            ],
        }
    }

    /// Every character that would end a row or a column has to leave the text,
    /// or a multi-line tooltip reads as several tags.
    #[test]
    fn text_that_would_break_a_row_is_escaped() {
        let cases = [
            ("Chest", "Chest"),
            ("Open {0}", "Open {0}"),
            ("first\nsecond", "first\\nsecond"),
            ("a\tb", "a\\tb"),
            ("a\r\nb", "a\\r\\nb"),
            ("back\\slash", "back\\\\slash"),
            ("", ""),
            ("\u{e9}l\u{e8}ve", "\u{e9}l\u{e8}ve"),
        ];

        for (text, expected) in cases {
            assert_eq!(escape(text), expected, "{text:?}");
        }
    }

    /// A row carries the id a mod would write down, then the two argument
    /// counts, then the text.
    #[test]
    fn a_table_opens_with_a_header_and_one_row_per_tag() {
        let tags = vec![
            LocaTag {
                arguments: 1,
                generic_arguments: 2,
                ..tag(122_361, "Open {0}")
            },
            tag(122_362, "line\nbreak"),
        ];

        let body = table(&tags);

        assert_eq!(
            body,
            format!(
                "id\targuments\tgeneric\ttext{EOL}\
                 122361\t1\t2\tOpen {{0}}{EOL}\
                 122362\t0\t0\tline\\nbreak{EOL}"
            )
        );
        assert_eq!(body.lines().count(), 3, "a header and one row per tag");
    }

    /// The crate names two language ids. Nothing here invents a name for one it
    /// does not.
    #[test]
    fn a_language_file_is_named_for_the_ids_the_crate_knows() {
        let cases = [
            (0, "keenglish.tsv"),
            (1, "english.tsv"),
            (2, "language-2.tsv"),
            (99, "language-99.tsv"),
        ];

        for (id, expected) in cases {
            assert_eq!(table_name(id), expected, "language {id}");
        }
    }

    /// With nothing named, every table is written, authoring language first and
    /// the rest in ascending id order however the resource listed them.
    #[test]
    fn every_language_is_selected_by_default_in_id_order() {
        let selected = select_languages(&collection(), &[]).expect("every language is present");

        let ids: Vec<u32> = selected.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, [0, 1, 2]);
        assert_eq!(selected[0].1, ContentHash::new(10, [0, 0, 0]));
        assert_eq!(selected[1].1, ContentHash::new(20, [0, 0, 0]));
    }

    /// A repeated id writes one file, and a named id the build does not ship is
    /// refused naming what it does ship.
    #[test]
    fn named_languages_are_deduplicated_and_an_absent_one_is_refused() {
        let selected = select_languages(&collection(), &[2, 1, 2]).expect("both are present");
        let ids: Vec<u32> = selected.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, [1, 2]);

        let err = select_languages(&collection(), &[7]).expect_err("language 7 is not shipped");
        let text = format!("{err:#}");
        assert!(text.contains("no language 7"), "{text}");
        assert!(
            text.contains("0, 2, 1"),
            "the refusal lists what is there: {text}"
        );
    }

    /// A run narrowed by `--language` writes fewer tables than a full one, so a
    /// table the previous run left behind would read as part of this one.
    #[test]
    fn a_replaced_extraction_keeps_no_table_from_the_run_before_it() {
        let dir = TestDir::new("loca-replace");
        let target = dir.path().join("23966345");
        dir.write("23966345/keenglish.tsv", b"old");
        dir.write("23966345/language-9.tsv", b"old");
        dir.write("23966345/notes.md", b"someone else's file");

        let cleared = prepare_target(&target).expect("the directory is prepared");

        assert_eq!(cleared, 2, "both tables are removed");
        assert!(!target.join("keenglish.tsv").exists());
        assert!(!target.join("language-9.tsv").exists());
        assert!(
            target.join("notes.md").is_file(),
            "only the extension this command writes is removed"
        );
    }

    /// A first run creates the directory and removes nothing.
    #[test]
    fn a_first_run_creates_the_directory() {
        let dir = TestDir::new("loca-first");
        let target = dir.path().join("fresh").join("23966345");

        let cleared = prepare_target(&target).expect("the directory is created");

        assert_eq!(cleared, 0);
        assert!(target.is_dir());
    }

    /// An archived build carries the executable and the directory alone. The
    /// refusal names the payload rather than failing inside a read.
    #[test]
    fn a_build_without_its_payload_is_refused_by_name() {
        let dir = TestDir::new("loca-archived");
        let exe = dir.write("archive/enshrouded.exe", b"MZ");
        dir.write("archive/enshrouded.kfc", b"KFC3");

        let err = require_container_set(&exe).expect_err("the payload is absent");
        let text = format!("{err:#}");
        assert!(text.contains("enshrouded.kfc_resources"), "{text}");
        assert!(text.contains("archived build"), "{text}");
    }

    /// Each file of the set is named on its own when it is the one missing.
    #[test]
    fn a_missing_executable_and_a_missing_directory_are_named_separately() {
        let dir = TestDir::new("loca-missing");
        let absent = dir.path().join("nothing").join("enshrouded.exe");
        let err = require_container_set(&absent).expect_err("no executable");
        assert!(
            format!("{err:#}").contains("no client executable"),
            "{err:#}"
        );

        let exe = dir.write("bare/enshrouded.exe", b"MZ");
        let err = require_container_set(&exe).expect_err("no directory");
        let text = format!("{err:#}");
        assert!(text.contains("no container directory"), "{text}");
        assert!(text.contains("enshrouded.kfc"), "{text}");
    }

    /// `SteamCMD` writes `steamapps` inside the install directory. A Steam
    /// library keeps one beside the whole `common` tree, which is where the
    /// manifest for an installed client sits.
    ///
    /// The manifest read is the client's own application, because the dedicated
    /// server is a different one and carries a buildid from a different series.
    #[test]
    fn a_buildid_is_read_from_either_manifest_layout() {
        const MANIFEST: &[u8] = br#""AppState"
{
	"buildid"		"23966345"
}
"#;
        let name = format!("appmanifest_{}.acf", crate::root::CLIENT_APP_ID);

        let fetched = TestDir::new("loca-build-fetched");
        let exe = fetched.write("install/enshrouded.exe", b"MZ");
        fetched.write(&format!("install/steamapps/{name}"), MANIFEST);
        assert_eq!(
            locate_build(&exe, None).expect("the manifest beside the install"),
            "23966345"
        );

        let library = TestDir::new("loca-build-library");
        let exe = library.write("steamapps/common/Enshrouded/enshrouded.exe", b"MZ");
        library.write(&format!("steamapps/{name}"), MANIFEST);
        assert_eq!(
            locate_build(&exe, None).expect("the manifest beside the common tree"),
            "23966345"
        );
    }

    /// The dedicated server's manifest is not the client's. Reading the wrong
    /// application would name a directory after a build the tables did not come
    /// from.
    #[test]
    fn the_dedicated_servers_manifest_does_not_name_a_client_build() {
        let dir = TestDir::new("loca-build-server-app");
        let exe = dir.write("install/enshrouded.exe", b"MZ");
        dir.write(
            &format!("install/steamapps/appmanifest_{}.acf", crate::root::APP_ID),
            br#""AppState"
{
	"buildid"		"23178631"
}
"#,
        );

        assert_ne!(
            crate::root::CLIENT_APP_ID,
            crate::root::APP_ID,
            "the client and the dedicated server are separate applications"
        );
        assert!(
            locate_build(&exe, None).is_err(),
            "a server manifest must not name the client build"
        );
    }

    /// The buildid becomes a directory name, so a typed one is held to the same
    /// rule wherever it came from, and a build with no manifest says so.
    #[test]
    fn a_typed_buildid_is_checked_and_an_absent_manifest_names_both_places() {
        let dir = TestDir::new("loca-build-escape");
        let exe = dir.write("install/enshrouded.exe", b"MZ");

        for name in ["..\\..\\out", "C:\\SteamLibrary", "/abs", ".fetching"] {
            assert!(locate_build(&exe, Some(name)).is_err(), "--build {name}");
        }
        assert_eq!(
            locate_build(&exe, Some("23178631")).expect("a plain buildid"),
            "23178631"
        );

        let err = locate_build(&exe, None).expect_err("no manifest either side");
        let text = format!("{err:#}");
        assert!(text.contains("--build"), "{text}");
        assert!(text.contains(" or "), "both places are named: {text}");
    }
}
