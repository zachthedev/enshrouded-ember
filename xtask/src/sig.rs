//! Following a hooked address from one build into the next.
//!
//! One subcommand per stage. `analyze` imports a build into a Ghidra project,
//! analyzes it and seeds a function at every program entry. `export` writes the
//! analyzed program out as a `BinExport` file. `diff` runs `BinDiff` over a pair of
//! those files. `read` prints what the result database proposes for an address.
//!
//! Seeding is why the rest works on an engine entry. A program record's entry
//! function is referenced only from the record in `.rdata`, so nothing calls
//! it, auto-analysis never reaches it, and an address the exporter does not
//! know as a function can never be matched. The entries come from the same walk
//! `schema extract` prints, so both commands read one list out of one image.
//!
//! **A proposal is a candidate, never an answer.** The differ matches call
//! graphs. Confirm every row against the binary itself, by the shape of the
//! call graph around it and by a string anchor, before it reaches a signature
//! row.
//!
//! Ghidra and `BinDiff` are large tools that no pin file installs and no runner
//! carries, so each stage resolves its tool from an environment variable and
//! names that variable when it is unset. Every stage costs minutes and hundreds
//! of megabytes, which is why this runs on a developer's machine rather than in
//! continuous integration.
//!
//! Nothing here is committed. Projects, exports, address lists and result
//! databases land under the gitignored `.cache` and are regenerated from a
//! build the caller fetched.

use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::SigCommand;
use crate::image::Image;
use crate::root::DevRoot;
use crate::ui::{Mark, Row, Ui};

/// The variable naming a Ghidra installation.
const GHIDRA_HOME: &str = "EMBER_GHIDRA_HOME";

/// The variable naming a `BinDiff` installation.
const BINDIFF_HOME: &str = "EMBER_BINDIFF_HOME";

/// The Ghidra class every headless stage drives.
const HEADLESS_CLASS: &str = "ghidra.app.util.headless.AnalyzeHeadless";

/// The directory holding the Ghidra scripts this module runs, under the
/// workspace root.
const SCRIPT_DIR: [&str; 2] = ["xtask", "ghidra"];

/// The script that creates a function at each seeded address.
const SEED_SCRIPT: &str = "SeedFunctions.java";

/// The script that writes the `BinExport` file.
const EXPORT_SCRIPT: &str = "ExportBinExport.java";

/// The file an analyze run writes its seed addresses to.
const SEED_FILE: &str = "seed-addresses.txt";

/// The server executable inside a fetched or archived build.
const SERVER_EXE: &str = "enshrouded_server.exe";

/// Java options every headless run carries.
///
/// The collector and the compiler are held to two threads each, so a run that
/// takes a quarter of an hour leaves the machine usable.
const VM_ARGS: &str = "-XX:ParallelGCThreads=2 -XX:CICompilerCount=2";

/// What a headless log holds once the program is on disk.
const SAVED: &str = "Save succeeded";

/// How often a bounded wait asks whether the child is done.
const POLL: Duration = Duration::from_millis(500);

/// How often a bounded wait says the run is still going.
const HEARTBEAT: Duration = Duration::from_secs(60);

/// The heap and the bound a headless stage runs under.
struct Headless<'a> {
    /// The Java heap ceiling, such as `4G`.
    heap: &'a str,
    /// Seconds the stage may take before it is stopped.
    timeout: u64,
}

/// Run one `sig` subcommand.
///
/// # Errors
///
/// Returns an error when a tool is not installed, a path refuses to resolve, an
/// image refuses to parse, or a stage exits non-zero or outlives its bound.
pub fn run(command: &SigCommand, root: &DevRoot, ui: &Ui) -> Result<bool> {
    match command {
        SigCommand::Analyze {
            build,
            server,
            address,
            heap,
            timeout,
            force,
        } => analyze(
            root,
            ui,
            build.as_deref(),
            server.as_deref(),
            address,
            &Headless {
                heap,
                timeout: *timeout,
            },
            *force,
        ),
        SigCommand::Export {
            build,
            server,
            heap,
            timeout,
        } => export(
            root,
            ui,
            build.as_deref(),
            server.as_deref(),
            &Headless {
                heap,
                timeout: *timeout,
            },
        ),
        SigCommand::Diff { old, new, timeout } => diff(root, ui, old, new, *timeout),
        SigCommand::Read {
            old,
            new,
            address,
            side,
            moved,
        } => read(root, ui, old, new, address, side, *moved),
    }
}

// ///////////////////////////////////////////////
// Stages
// ///////////////////////////////////////////////

/// Import a build, analyze it, and seed a function at every program entry.
fn analyze(
    root: &DevRoot,
    ui: &Ui,
    build: Option<&str>,
    server: Option<&Path>,
    extra: &[String],
    headless: &Headless<'_>,
    force: bool,
) -> Result<bool> {
    let launcher = ghidra_launcher()?;
    let build_id = root.build_id(build)?;
    let exe = locate_server(root, &build_id, server)?;
    let project = root.ghidra_build_dir(&build_id)?;

    // Ghidra refuses an import into a project that already holds the program,
    // a quarter of a minute into the run. The same refusal here costs nothing
    // and says what the flag is.
    if !force && project_file(&project, &build_id).is_file() {
        bail!(
            "{} already holds a Ghidra project for build {build_id}. Pass --force to import over \
             it, which discards the analysis in it.",
            project.display()
        );
    }

    ui.section("sig analyze");
    ui.line(&format!("build {build_id}"));
    ui.line(&format!("reading {}", exe.display()));

    let image = Image::open(&exe)?;
    let seeds = seed_addresses(&image, extra)?;
    std::fs::create_dir_all(&project).with_context(|| format!("creating {}", project.display()))?;
    let list = project.join(SEED_FILE);
    std::fs::write(&list, seed_list(&seeds))
        .with_context(|| format!("writing {}", list.display()))?;
    ui.rows(&[
        Row::new(Mark::Note, "program entries", seeds.len().to_string())
            .note("a function is created at each one before the export runs"),
        Row::new(Mark::Note, "named by hand", extra.len().to_string())
            .note("--address, for a target no program record names"),
    ]);

    let mut args = vec![
        project.display().to_string(),
        build_id.clone(),
        "-import".to_string(),
        exe.display().to_string(),
        "-max-cpu".to_string(),
        "8".to_string(),
    ];
    if force {
        args.push("-overwrite".to_string());
    }
    args.extend(post_script(SEED_SCRIPT, &list.display().to_string()));

    let log = project.join("analyze.log");
    let status = launch(ui, &launcher, headless, &args, &project, &log, "analysis")?;
    finish(ui, status, &log, "analysis", true)
}

/// Write the analyzed program out as a `BinExport` file.
fn export(
    root: &DevRoot,
    ui: &Ui,
    build: Option<&str>,
    server: Option<&Path>,
    headless: &Headless<'_>,
) -> Result<bool> {
    let launcher = ghidra_launcher()?;
    let build_id = root.build_id(build)?;
    let exe = locate_server(root, &build_id, server)?;
    let project = root.ghidra_build_dir(&build_id)?;
    if !project.is_dir() {
        bail!(
            "no Ghidra project at {}. Run `cargo xtask sig analyze --build {build_id}` first.",
            project.display()
        );
    }
    let out = export_path(&project, &build_id);

    ui.section("sig export");
    ui.line(&format!("build {build_id}"));

    let mut args = vec![
        project.display().to_string(),
        build_id.clone(),
        "-process".to_string(),
        program_name(&exe),
        "-noanalysis".to_string(),
        "-readOnly".to_string(),
    ];
    args.extend(post_script(EXPORT_SCRIPT, &out.display().to_string()));

    let log = project.join("export.log");
    let status = launch(ui, &launcher, headless, &args, &project, &log, "export")?;
    let passed = finish(ui, status, &log, "export", false)?;
    if !passed {
        return Ok(false);
    }
    // A headless run whose script threw still exits zero, so the file the
    // script was asked for is what says the stage worked.
    if !out.is_file() {
        bail!(
            "the export run exited zero and wrote no {}. Read {}.",
            out.display(),
            log.display()
        );
    }
    ui.rows(&[
        Row::new(Mark::Ok, "written", crate::ui::bytes(size_of_file(&out)))
            .note(out.display().to_string()),
    ]);
    Ok(true)
}

/// Diff a pair of exports, with the known build as the primary.
fn diff(root: &DevRoot, ui: &Ui, old: &str, new: &str, timeout: u64) -> Result<bool> {
    let bindiff = bindiff_exe()?;
    let primary = export_path(&root.ghidra_build_dir(old)?, old);
    let secondary = export_path(&root.ghidra_build_dir(new)?, new);
    for (label, path) in [("old", &primary), ("new", &secondary)] {
        if !path.is_file() {
            bail!(
                "no export for the {label} build at {}. Run `cargo xtask sig export` for it first.",
                path.display()
            );
        }
    }
    let pair = root.bindiff_pair_dir(old, new)?;

    ui.section("sig diff");
    ui.line(&format!("{old} to {new}"));

    // The primary is the build whose addresses are known, so the proposal for
    // the new build lands in the result's second address column.
    let args = [
        format!("--primary={}", primary.display()),
        format!("--secondary={}", secondary.display()),
        format!("--output_dir={}", pair.display()),
        "--output_format=bin".to_string(),
    ];
    let log = pair.join("bindiff.log");
    let started = Instant::now();
    let mut child = spawn(&bindiff, &args, &pair, &log)?;
    let status = bounded(ui, &mut child, "diff", timeout, &log)?;
    if !status.success() {
        ui.rows(&[
            Row::new(Mark::Fail, "diff", status_text(status)).note(log.display().to_string())
        ]);
        return Ok(false);
    }

    // The differ prints tens of thousands of per-block complaints that mean
    // nothing here, so the result database is what says the run worked.
    let result = result_path(&pair, old, new);
    if !result.is_file() {
        bail!(
            "BinDiff exited zero and wrote no {}. Read {}.",
            result.display(),
            log.display()
        );
    }
    ui.rows(&[
        Row::new(Mark::Ok, "result", crate::ui::bytes(size_of_file(&result)))
            .note(result.display().to_string()),
        Row::new(
            Mark::Note,
            "seconds",
            format!("{:.1}", started.elapsed().as_secs_f64()),
        ),
    ]);
    Ok(true)
}

/// Print what the result database proposes for each address.
fn read(
    root: &DevRoot,
    ui: &Ui,
    old: &str,
    new: &str,
    address: &[String],
    side: &str,
    moved: u32,
) -> Result<bool> {
    let bun = crate::spawn::resolve("bun")
        .map_err(anyhow::Error::msg)
        .context("bun is not installed, and `sig read` reads the result database with it")?;
    let pair = root.bindiff_pair_dir(old, new)?;
    let result = result_path(&pair, old, new);
    if !result.is_file() {
        bail!(
            "no result at {}. Run `cargo xtask sig diff {old} {new}` first.",
            result.display()
        );
    }
    for text in address {
        parse_address(text)?;
    }

    ui.section("sig read");
    let workspace = crate::workspace_root();
    let mut args = vec![
        "run".to_string(),
        workspace
            .join("tools")
            .join("bindiff-read.ts")
            .display()
            .to_string(),
        result.display().to_string(),
        "--side".to_string(),
        side.to_string(),
    ];
    if moved > 0 {
        args.push("--moved".to_string());
        args.push(moved.to_string());
    }
    args.extend(address.iter().cloned());

    let status = crate::spawn::command(&bun)
        .map_err(anyhow::Error::msg)?
        .args(&args)
        .current_dir(&workspace)
        .status()
        .with_context(|| format!("running {} {}", bun.display(), args.join(" ")))?;
    Ok(status.success())
}

// ///////////////////////////////////////////////
// Seeding
// ///////////////////////////////////////////////

/// Every address a function is created at, sorted and without repeats.
///
/// The program entries come from the image itself rather than from a dump on
/// disk, so this stage needs no extraction to have run first and cannot read a
/// stale one. A record whose entry is zero belongs to the other build and names
/// no address here.
fn seed_addresses(image: &Image, extra: &[String]) -> Result<Vec<u64>> {
    let mut found: Vec<u64> = crate::schema::program::walk(image)
        .into_iter()
        .map(|record| record.entry)
        .filter(|entry| *entry != 0)
        .collect();
    for text in extra {
        found.push(parse_address(text)?);
    }
    found.sort_unstable();
    found.dedup();
    Ok(found)
}

/// Read one address, with or without the `0x` prefix.
///
/// # Errors
///
/// Returns an error naming the text that is not an address.
fn parse_address(text: &str) -> Result<u64> {
    let digits = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .unwrap_or(text);
    u64::from_str_radix(digits, 16)
        .with_context(|| format!("{text:?} is not a hexadecimal address"))
}

/// The address list the seeding script reads.
fn seed_list(addresses: &[u64]) -> String {
    let mut out = String::from("# Auto-generated by `cargo xtask sig analyze`. Do not edit.\n");
    for address in addresses {
        let _ = writeln!(out, "{address:#x}");
    }
    out
}

// ///////////////////////////////////////////////
// Tools and paths
// ///////////////////////////////////////////////

/// The Ghidra launcher this host carries.
///
/// Ghidra's own `analyzeHeadless` wrapper pins the heap at 2G with no way to
/// raise it from the environment, so every stage calls the launcher underneath
/// it and passes the ceiling itself.
fn ghidra_launcher() -> Result<PathBuf> {
    let home = tool_home(
        GHIDRA_HOME,
        "a Ghidra installation",
        std::env::var_os(GHIDRA_HOME),
    )?;
    under(
        &home,
        GHIDRA_HOME,
        &Path::new("support").join(launcher_name()),
    )
}

/// The `BinDiff` differ this host carries.
fn bindiff_exe() -> Result<PathBuf> {
    let home = tool_home(
        BINDIFF_HOME,
        "a BinDiff installation",
        std::env::var_os(BINDIFF_HOME),
    )?;
    under(&home, BINDIFF_HOME, &Path::new("bin").join(bindiff_name()))
}

/// The directory a tool variable names.
///
/// The value is passed in rather than read here, so a case can drive every
/// refusal without touching the environment the rest of the run reads.
///
/// # Errors
///
/// Returns an error naming the variable when it is unset or names no directory.
fn tool_home(variable: &str, what: &str, value: Option<OsString>) -> Result<PathBuf> {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        bail!(
            "{variable} is not set. Point it at {what}, which this machine installs rather than \
             a pin file, because no runner carries one."
        );
    };
    let path = PathBuf::from(value);
    if !path.is_dir() {
        bail!(
            "{variable} names {}, which is not a directory",
            path.display()
        );
    }
    Ok(path)
}

/// One file a tool home has to hold.
///
/// # Errors
///
/// Returns an error naming the variable and the file when it is not there.
fn under(home: &Path, variable: &str, relative: &Path) -> Result<PathBuf> {
    let path = home.join(relative);
    if !path.is_file() {
        bail!(
            "{variable} names {}, which holds no {}",
            home.display(),
            relative.display()
        );
    }
    Ok(path)
}

/// The Ghidra launcher's file name on this host.
fn launcher_name() -> &'static str {
    if cfg!(windows) {
        "launch.bat"
    } else {
        "launch.sh"
    }
}

/// The `BinDiff` differ's file name on this host.
fn bindiff_name() -> &'static str {
    if cfg!(windows) {
        "bindiff.exe"
    } else {
        "bindiff"
    }
}

/// The executable a stage reads, fetched or archived.
///
/// A build the dev loop fetched sits under `server`, and a build pulled out of
/// the archive sits under `archive` keyed by its depot manifest gid. Both are
/// ordinary inputs here, so both are looked for before anything is refused.
fn locate_server(root: &DevRoot, build: &str, server: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = server {
        if !path.is_file() {
            bail!("no executable at {}", path.display());
        }
        return Ok(path.to_path_buf());
    }
    let fetched = root.build_dir(build)?.join(SERVER_EXE);
    if fetched.is_file() {
        return Ok(fetched);
    }
    let archived = root.archive_build_dir(build)?.join(SERVER_EXE);
    if archived.is_file() {
        return Ok(archived);
    }
    bail!(
        "no {SERVER_EXE} for build {build}, at {} or at {}. Fetch it with \
         `cargo xtask server fetch`, pull it with `bun run tools/archive.ts pull`, or name one \
         with --server.",
        fetched.display(),
        archived.display()
    )
}

/// The name Ghidra knows an imported executable by, which is its file name.
fn program_name(exe: &Path) -> String {
    exe.file_name().map_or_else(
        || SERVER_EXE.to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// The `BinExport` file one build's project holds.
fn export_path(project: &Path, build: &str) -> PathBuf {
    project.join(format!("{build}.BinExport"))
}

/// The file Ghidra writes for a project, which says the project is there.
///
/// The directory is no answer: this command creates it to hold the address
/// list before Ghidra runs at all.
fn project_file(project: &Path, build: &str) -> PathBuf {
    project.join(format!("{build}.gpr"))
}

/// The result database a pair's directory holds.
///
/// `BinDiff` names its result after the two input file stems, which are the two
/// buildids.
fn result_path(pair: &Path, old: &str, new: &str) -> PathBuf {
    pair.join(format!("{old}_vs_{new}.BinDiff"))
}

/// The script arguments a headless run ends with.
fn post_script(script: &str, argument: &str) -> Vec<String> {
    let dir = SCRIPT_DIR
        .iter()
        .fold(crate::workspace_root(), |path, part| path.join(part));
    vec![
        "-scriptPath".to_string(),
        dir.display().to_string(),
        "-postScript".to_string(),
        script.to_string(),
        argument.to_string(),
    ]
}

/// A file's size, or zero when it cannot be asked.
fn size_of_file(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |found| found.len())
}

// ///////////////////////////////////////////////
// Running a tool
// ///////////////////////////////////////////////

/// Start one headless Ghidra run and wait for it, bounded.
fn launch(
    ui: &Ui,
    launcher: &Path,
    headless: &Headless<'_>,
    tail: &[String],
    working_dir: &Path,
    log: &Path,
    label: &str,
) -> Result<ExitStatus> {
    let mut args = vec![
        "fg".to_string(),
        "jdk".to_string(),
        "Ghidra-Headless".to_string(),
        headless.heap.to_string(),
        VM_ARGS.to_string(),
        HEADLESS_CLASS.to_string(),
    ];
    args.extend(tail.iter().cloned());
    let mut child = spawn(launcher, &args, working_dir, log)?;
    bounded(ui, &mut child, label, headless.timeout, log)
}

/// Start a child with its output in a pair of logs beside the work.
///
/// A JVM crash writes its report into the working directory, so the working
/// directory is the one holding the logs rather than the repository.
fn spawn(program: &Path, args: &[String], working_dir: &Path, log: &Path) -> Result<Child> {
    std::fs::create_dir_all(working_dir)
        .with_context(|| format!("creating {}", working_dir.display()))?;
    let out = std::fs::File::create(log).with_context(|| format!("creating {}", log.display()))?;
    let error_path = error_log(log);
    let err = std::fs::File::create(&error_path)
        .with_context(|| format!("creating {}", error_path.display()))?;
    // The Ghidra launcher is a batch file, and cmd.exe runs the bare java it
    // names from the working directory ahead of PATH unless this is set.
    crate::spawn::command(program)
        .map_err(anyhow::Error::msg)?
        .env("NoDefaultCurrentDirectoryInExePath", "1")
        .args(args)
        .current_dir(working_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err))
        .spawn()
        .with_context(|| format!("running {}", program.display()))
}

/// The second log, holding what the child wrote to standard error.
fn error_log(log: &Path) -> PathBuf {
    log.with_extension("err.log")
}

/// Wait for a child, saying how long it has been going and stopping it at the
/// bound.
///
/// Every stage here costs minutes, and one that has stopped making progress
/// looks exactly like one that has not. The bound is what keeps a wedged tool
/// from holding the machine until somebody notices.
fn bounded(
    ui: &Ui,
    child: &mut Child,
    label: &str,
    timeout: u64,
    log: &Path,
) -> Result<ExitStatus> {
    let limit = Duration::from_secs(timeout);
    let started = Instant::now();
    let mut spoken = Duration::ZERO;
    ui.line(&format!(
        "{label} running, at most {timeout}s, output in {}",
        log.display()
    ));
    loop {
        if let Some(status) = child
            .try_wait()
            .context("asking whether the run finished")?
        {
            ui.line(&format!(
                "{label} finished after {:.0}s",
                started.elapsed().as_secs_f64()
            ));
            return Ok(status);
        }
        let elapsed = started.elapsed();
        if elapsed >= limit {
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "{label} outlived its {timeout}s bound and was stopped. Read {}, and raise \
                 --timeout when the machine is simply busy.",
                log.display()
            );
        }
        if elapsed.saturating_sub(spoken) >= HEARTBEAT {
            spoken = elapsed;
            ui.line(&format!(
                "{label} still running at {:.0}s",
                elapsed.as_secs_f64()
            ));
        }
        std::thread::sleep(POLL);
    }
}

/// Report one headless run, and say whether it passed.
///
/// A headless run exits zero when a post script throws, so the log is read for
/// the line the tool prints when it actually wrote something.
fn finish(ui: &Ui, status: ExitStatus, log: &Path, label: &str, saved: bool) -> Result<bool> {
    let text = read_logs(log);
    if !status.success() {
        ui.rows(
            &[Row::new(Mark::Fail, label, status_text(status)).note(log.display().to_string())],
        );
        return Ok(false);
    }
    if saved && !text.contains(SAVED) {
        bail!(
            "the {label} run exited zero and its log holds no {SAVED:?}, so nothing was written \
             to the project. Read {}.",
            log.display()
        );
    }
    let mut rows = vec![Row::new(Mark::Ok, label, "exit 0").note(log.display().to_string())];
    rows.extend(script_lines(&text));
    ui.rows(&rows);
    Ok(true)
}

/// The one line each script prints, which is the part of a headless log worth
/// putting on the screen.
fn script_lines(text: &str) -> Vec<Row> {
    let mut rows = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        for (prefix, name) in [("INFO  seed ", "seeded"), ("INFO  binexport ", "exported")] {
            if let Some(rest) = trimmed.strip_prefix(prefix) {
                rows.push(Row::new(Mark::Note, name, trim_ghidra(rest)));
            }
        }
    }
    rows
}

/// Drop the source tag Ghidra puts after every script line.
fn trim_ghidra(line: &str) -> String {
    line.split_once(" (Headless")
        .map_or(line, |(text, _)| text)
        .trim()
        .to_string()
}

/// Both logs of a run, as one string.
fn read_logs(log: &Path) -> String {
    let mut text = std::fs::read_to_string(log).unwrap_or_default();
    text.push_str(&std::fs::read_to_string(error_log(log)).unwrap_or_default());
    text
}

/// How an exit status reads in a result row.
fn status_text(status: ExitStatus) -> String {
    status
        .code()
        .map_or_else(|| "killed".to_string(), |code| format!("exit {code}"))
}

#[cfg(test)]
mod tests {
    use super::{
        error_log, export_path, parse_address, project_file, result_path, script_lines, seed_list,
        tool_home, trim_ghidra,
    };
    use crate::testutil::TestDir;
    use std::ffi::OsString;
    use std::path::Path;

    /// The seeding script skips the header, so the list can carry the line
    /// every generated file carries.
    #[test]
    fn the_address_list_opens_with_the_generated_header() {
        let text = seed_list(&[0x1_4006_c1a0, 0x1_401a_ba90]);
        let mut lines = text.lines();
        assert_eq!(
            lines.next(),
            Some("# Auto-generated by `cargo xtask sig analyze`. Do not edit.")
        );
        assert_eq!(lines.next(), Some("0x14006c1a0"));
        assert_eq!(lines.next(), Some("0x1401aba90"));
        assert_eq!(lines.next(), None);
    }

    /// An address reaches a Ghidra script, so a value that is not one is
    /// refused here rather than becoming a seed nothing can parse.
    ///
    /// `0b1010` is not in the refused set. Every character in it is a
    /// hexadecimal digit, so a bare address is read as one, and a reader who
    /// meant binary gets `0xb1010`. The prefix this format has is `0x`.
    #[test]
    fn an_address_is_hexadecimal_with_or_without_the_prefix() {
        assert_eq!(parse_address("0x14006c1a0").unwrap(), 0x1_4006_c1a0);
        assert_eq!(parse_address("0X14006C1A0").unwrap(), 0x1_4006_c1a0);
        assert_eq!(parse_address("14006c1a0").unwrap(), 0x1_4006_c1a0);
        assert_eq!(parse_address("0b1010").unwrap(), 0x000b_1010);
        for bad in ["", "0x", "zz", "0x14006c1a0z", "-1", "1400 6c1a0"] {
            assert!(parse_address(bad).is_err(), "address {bad:?}");
        }
    }

    /// An analyze run refuses a project that is already there, and the
    /// directory is not what says one is. This command makes the directory
    /// itself, to hold the address list, before Ghidra has run at all.
    #[test]
    fn a_project_is_recognized_by_its_own_file_rather_than_its_directory() {
        let dir = TestDir::new("sig-project");
        let project = dir.path().join("23178631");
        std::fs::create_dir_all(&project).expect("the directory is made");
        dir.write("23178631/seed-addresses.txt", b"# empty");

        assert!(
            !project_file(&project, "23178631").is_file(),
            "a directory holding this command's own files is not a project"
        );

        dir.write("23178631/23178631.gpr", b"");
        assert!(
            project_file(&project, "23178631").is_file(),
            "the project file is what says Ghidra has a project here"
        );
    }

    /// `BinDiff` names its result after the two input stems, so the name this
    /// module writes and the name it later reads have to agree.
    #[test]
    fn the_pair_result_is_named_for_both_exports() {
        let export = export_path(&Path::new("root").join("23178631"), "23178631");
        assert_eq!(export.file_name().unwrap(), "23178631.BinExport");
        let result = result_path(Path::new("pair"), "23178631", "5177045887918896292");
        assert_eq!(
            result.file_name().unwrap(),
            "23178631_vs_5177045887918896292.BinDiff"
        );
    }

    /// The second log is a sibling of the first rather than a replacement for
    /// it, because what a headless run says about a failure often lands only on
    /// standard error.
    #[test]
    fn the_error_log_sits_beside_the_log() {
        let log = Path::new("dir").join("analyze.log");
        assert_eq!(error_log(&log), Path::new("dir").join("analyze.err.log"));
    }

    /// An unset variable, an empty one and one naming nothing are three
    /// different mistakes, and each refusal has to name the variable, because
    /// they are fixed in different places.
    #[test]
    fn a_tool_home_that_names_no_directory_is_refused_by_name() {
        let cases: [(Option<OsString>, &str); 3] = [
            (None, "is not set"),
            (Some(OsString::new()), "is not set"),
            (
                Some(OsString::from("Z:\\nothing-is-installed-here")),
                "not a directory",
            ),
        ];
        for (value, wanted) in cases {
            let err = tool_home("A_TEST_TOOL_HOME", "a test tool", value).expect_err(wanted);
            let text = format!("{err:#}");
            assert!(text.contains("A_TEST_TOOL_HOME"), "{text}");
            assert!(text.contains(wanted), "{text}");
        }
    }

    /// Both scripts report through the log, so a run says what it seeded and
    /// what it wrote without anyone opening the file.
    #[test]
    fn each_script_line_reaches_a_result_row() {
        let log = concat!(
            "INFO  Using log config file (Headless)\n",
            "INFO  seed read=3 already=1 created=2 refused=0 (HeadlessAnalyzer)\n",
            "INFO  binexport wrote out.BinExport bytes=66394294 functions=26291 (HeadlessAnalyzer)\n",
            "INFO  Save succeeded (HeadlessAnalyzer)\n",
        );
        let rows = script_lines(log);
        assert_eq!(
            rows.len(),
            2,
            "the seed line and the export line, and no other"
        );
    }

    #[test]
    fn a_script_line_drops_ghidras_own_tag() {
        assert_eq!(
            trim_ghidra("read=1 created=1 (HeadlessAnalyzer)"),
            "read=1 created=1"
        );
        assert_eq!(trim_ghidra("read=1 created=1"), "read=1 created=1");
    }
}
