//! Fetch, seed, launch, tail and stop a dedicated server build.
//!
//! Everything the loop controls lives in a run directory beside the fetched
//! build: the save, the server's own logs, the config, the captured output and
//! the pid file. The server runs with that directory as its working directory,
//! so its relative defaults land there as well. A build can be deleted and
//! refetched without losing a test world.
//!
//! One set of files still lands in the build directory. The Steamworks client
//! library resolves `logs` and `config` against the executable rather than the
//! working directory, so a run leaves `logs/connection_log.txt` and friends
//! beside `enshrouded_server.exe`. Deleting the build directory and running
//! `server fetch` again restores anything that matters, and the loop writes
//! nothing there itself.
//!
//! The pid file is a hint, never an identity. Every command that reads it asks
//! the operating system what the pid is running and compares that to the
//! server in the build directory before it signals, waits on or kills anything.
//! A pid that is alive and is something else is a stale file, deleted and named.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::ServerCommand;
use crate::proc::{self, Claim, Exit};
use crate::root::{APP_ID, DevRoot, build_id_from_manifest};
use crate::ui::{Mark, Row, Ui};

/// The server executable inside a build directory.
const SERVER_EXE: &str = "enshrouded_server.exe";

/// The server's configuration file.
const CONFIG: &str = "enshrouded_server.json";

/// The file recording the last launched process id.
const PID_FILE: &str = "server.pid";

/// The captured output of the last launch.
const LOG_FILE: &str = "server.log";

/// Seconds to wait for a clean stop when none is named.
pub const DEFAULT_STOP_TIMEOUT: u64 = 60;

/// Lines of log to print when none is named.
pub const DEFAULT_TAIL: usize = 40;

/// How often a follow re-reads the log.
const FOLLOW_INTERVAL: Duration = Duration::from_millis(500);

/// How long a launch waits before asking whether the child is still there.
///
/// A server that cannot bind its port, parse its config or find its data exits
/// inside this window. One that survives it has reached its own startup.
const STARTUP_GRACE: Duration = Duration::from_millis(1500);

/// Lines of log shown when the server exits during the grace period.
const EARLY_EXIT_TAIL: usize = 12;

/// Run one `server` subcommand.
///
/// # Errors
///
/// Returns an error when a path refuses to resolve, a tool is missing, or a
/// process cannot be started or signaled.
pub fn run(command: &ServerCommand, root: &DevRoot, ui: &Ui) -> Result<bool> {
    match command {
        ServerCommand::Fetch { build } => fetch(root, ui, build.as_deref()),
        ServerCommand::Seed {
            fixture,
            build,
            force,
        } => seed(root, ui, fixture, build.as_deref(), *force),
        ServerCommand::Run {
            build,
            inject,
            wait,
        } => launch(root, ui, build.as_deref(), inject.as_deref(), *wait),
        ServerCommand::Logs {
            build,
            follow,
            lines,
        } => logs(root, ui, build.as_deref(), *follow, *lines),
        ServerCommand::Stop {
            build,
            timeout,
            force,
            send_break: _,
        } => stop(root, ui, build.as_deref(), *timeout, *force),
    }
}

// ///////////////////////////////////////////////
// Fetch
// ///////////////////////////////////////////////

/// Pull the branch head into a directory named by the buildid it turns out to
/// be.
fn fetch(root: &DevRoot, ui: &Ui, expected: Option<&str>) -> Result<bool> {
    let steamcmd = root.steamcmd();
    if !steamcmd.is_file() {
        bail!(
            "no SteamCMD at {}. Download it from \
             https://steamcdn-a.akamaihd.net/client/installer/steamcmd.zip and unpack it there.",
            steamcmd.display()
        );
    }
    if let Some(name) = expected {
        let target = root.build_dir(name)?;
        if target.exists() {
            bail!(
                "{} already holds build {name}. Delete it to refetch, or name another buildid.",
                target.display()
            );
        }
    }

    let staging = root.server_dir().join(".fetching");
    crate::steam::ensure_outside_library(&staging, "a server directory")?;
    std::fs::create_dir_all(&staging).with_context(|| format!("creating {}", staging.display()))?;

    ui.section("server fetch");
    ui.line(&format!("app {APP_ID} into {}", staging.display()));
    let status = Command::new(&steamcmd)
        .args([
            "+force_install_dir".into(),
            staging.as_os_str().to_os_string(),
            "+login".into(),
            "anonymous".into(),
            "+app_update".into(),
            APP_ID.into(),
            "validate".into(),
            "+quit".into(),
        ])
        .status()
        .with_context(|| format!("running {}", steamcmd.display()))?;
    if !status.success() {
        bail!(
            "SteamCMD exited with {}. The partial download is at {}.",
            status
                .code()
                .map_or_else(|| "no exit code".to_string(), |code| format!("code {code}")),
            staging.display()
        );
    }

    let actual = build_id_from_manifest(&staging, APP_ID)?;
    let target = root.build_dir(&actual)?;
    if target.exists() {
        bail!(
            "{} already holds build {actual}. The fetch is at {}; delete one of the two.",
            target.display(),
            staging.display()
        );
    }
    std::fs::rename(&staging, &target)
        .with_context(|| format!("naming the fetch {}", target.display()))?;

    let exe = target.join(SERVER_EXE);
    let size = std::fs::metadata(&exe).map_or(0, |meta| meta.len());
    let mut rows = vec![
        Row::new(Mark::Ok, "buildid", &actual),
        Row::new(Mark::Ok, SERVER_EXE, crate::ui::bytes(size)),
    ];
    match crate::kfc::read_beside(&exe) {
        Ok(revision) => rows.push(
            Row::new(Mark::Ok, "revision", format!("r{}", revision.number)).note(revision.branch),
        ),
        Err(err) => rows.push(Row::new(Mark::Warn, "revision", "unknown").note(err.to_string())),
    }
    ui.rows(&rows);
    ui.line(&format!("fetched to {}", target.display()));

    if let Some(name) = expected
        && name != actual
    {
        bail!(
            "asked for build {name}, but Steam served build {actual}, now at {}",
            target.display()
        );
    }
    Ok(true)
}

// ///////////////////////////////////////////////
// Seed
// ///////////////////////////////////////////////

/// Copy a fixture save and config into a build's run directory.
fn seed(root: &DevRoot, ui: &Ui, fixture: &Path, build: Option<&str>, force: bool) -> Result<bool> {
    if !fixture.is_dir() {
        bail!("no fixture directory at {}", fixture.display());
    }
    let build_id = root.build_id(build)?;
    let run = root.run_dir(&build_id)?;
    let saves = run.join("savegame");
    refuse_fixture_inside_run(fixture, &run)?;

    // The source is settled before anything is removed, so a fixture that
    // turns out to be the run directory cannot become a copy of a directory
    // into its own child.
    let source = if fixture.join("savegame").is_dir() {
        fixture.join("savegame")
    } else {
        fixture.to_path_buf()
    };

    if saves.exists() && !force {
        bail!(
            "{} already holds a save. Pass --force to replace it.",
            saves.display()
        );
    }
    if saves.exists() {
        std::fs::remove_dir_all(&saves).with_context(|| format!("removing {}", saves.display()))?;
    }
    std::fs::create_dir_all(&run).with_context(|| format!("creating {}", run.display()))?;
    let files = copy_tree(&source, &saves)?;

    ui.section("server seed");
    let mut rows = vec![
        Row::new(Mark::Ok, "build", &build_id),
        Row::new(Mark::Ok, "savegame", format!("{files} files")).note(saves.display().to_string()),
    ];
    let fixture_config = fixture.join(CONFIG);
    if fixture_config.is_file() {
        let target = run.join(CONFIG);
        std::fs::copy(&fixture_config, &target).with_context(|| {
            format!(
                "copying {} to {}",
                fixture_config.display(),
                target.display()
            )
        })?;
        rows.push(Row::new(Mark::Ok, CONFIG, "copied").note(target.display().to_string()));
    } else {
        rows.push(
            Row::new(Mark::Note, CONFIG, "not in the fixture")
                .note("`server run` writes one from the shipped defaults"),
        );
    }
    ui.rows(&rows);
    Ok(true)
}

/// Refuse a fixture that is the run directory or sits inside it.
///
/// Seeding copies the fixture into the run directory and, with `--force`, first
/// removes the save that is there. A fixture inside the run directory would be
/// deleted, copied into its own child, or both.
fn refuse_fixture_inside_run(fixture: &Path, run: &Path) -> Result<()> {
    let fixture_real = std::fs::canonicalize(fixture).unwrap_or_else(|_| fixture.to_path_buf());
    let run_real = std::fs::canonicalize(run).unwrap_or_else(|_| run.to_path_buf());
    if fixture_real.starts_with(&run_real) {
        bail!(
            "the fixture {} is inside the run directory {}, which seeding overwrites",
            fixture.display(),
            run.display()
        );
    }
    Ok(())
}

/// Copy a directory tree, returning how many files were written.
///
/// An entry whose path is the destination itself is skipped, which is what
/// keeps a copy from descending into what it is writing.
fn copy_tree(from: &Path, to: &Path) -> Result<usize> {
    std::fs::create_dir_all(to).with_context(|| format!("creating {}", to.display()))?;
    let to_real = std::fs::canonicalize(to).unwrap_or_else(|_| to.to_path_buf());
    let mut count = 0;
    for entry in std::fs::read_dir(from).with_context(|| format!("reading {}", from.display()))? {
        let entry = entry?;
        let source = entry.path();
        if std::fs::canonicalize(&source).is_ok_and(|real| real == to_real) {
            continue;
        }
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            count += copy_tree(&source, &target)?;
        } else {
            std::fs::copy(&source, &target)
                .with_context(|| format!("copying {} to {}", source.display(), target.display()))?;
            count += 1;
        }
    }
    Ok(count)
}

// ///////////////////////////////////////////////
// Run
// ///////////////////////////////////////////////

/// Launch a build, optionally injecting a library into the running process.
fn launch(
    root: &DevRoot,
    ui: &Ui,
    build: Option<&str>,
    inject: Option<&Path>,
    wait: bool,
) -> Result<bool> {
    let build_id = root.build_id(build)?;
    let dir = root.build_dir(&build_id)?;
    let exe = dir.join(SERVER_EXE);
    if !exe.is_file() {
        bail!(
            "no server executable at {}. Run `cargo xtask server fetch` first.",
            exe.display()
        );
    }
    let run = root.run_dir(&build_id)?;
    ui.section("server run");
    match recorded_server(&run, &exe)? {
        Recorded::Ours(server) => bail!(
            "build {build_id} is already running as process {}. Run `cargo xtask server stop` first.",
            server.pid()
        ),
        Recorded::Stale { pid, image } => ui.line(&format!(
            "removed a stale pid file: process {pid} is {}, not the server",
            image.display()
        )),
        Recorded::Gone(_) | Recorded::None => {}
    }

    std::fs::create_dir_all(run.join("savegame"))
        .with_context(|| format!("creating {}", run.join("savegame").display()))?;
    std::fs::create_dir_all(run.join("logs"))
        .with_context(|| format!("creating {}", run.join("logs").display()))?;
    let config = run.join(CONFIG);
    let wrote_config = !config.is_file();
    if wrote_config {
        std::fs::write(&config, default_config(&run)?)
            .with_context(|| format!("writing {}", config.display()))?;
    }

    let log = run.join(LOG_FILE);
    let args = vec![
        "--config".to_string(),
        config.to_string_lossy().into_owned(),
    ];
    // The child handle pins the new pid for as long as this function runs, so
    // nothing below can act on a recycled id.
    let mut child = proc::spawn(&exe, &args, &run, &log)?;
    let pid = child.id();
    std::fs::write(run.join(PID_FILE), pid.to_string())
        .with_context(|| format!("writing {}", run.join(PID_FILE).display()))?;

    std::thread::sleep(STARTUP_GRACE);
    if let Some(status) = child
        .try_wait()
        .context("asking whether the server is still running")?
    {
        let _ = std::fs::remove_file(run.join(PID_FILE));
        ui.rows(&[Row::new(Mark::Fail, "process", pid.to_string())
            .note(format!("exited during startup with {status}"))]);
        for line in tail_of(&log, EARLY_EXIT_TAIL)? {
            ui.always(&line);
        }
        ui.line(&format!("the whole output is at {}", log.display()));
        return Ok(false);
    }

    let mut rows = vec![
        Row::new(Mark::Ok, "build", &build_id),
        Row::new(Mark::Ok, "process", pid.to_string()),
        Row::new(Mark::Ok, "log", log.display().to_string()),
    ];
    if wrote_config {
        rows.push(
            Row::new(Mark::Warn, CONFIG, "written")
                .note("bound to 127.0.0.1 with one slot; user group passwords were generated"),
        );
    }
    match inject {
        Some(library) => match inject_library(pid, library) {
            Ok(()) => rows.push(Row::new(
                Mark::Ok,
                "injected",
                library.display().to_string(),
            )),
            Err(err) => {
                rows.push(Row::new(Mark::Fail, "injected", "failed").note(format!("{err:#}")));
                ui.rows(&rows);
                return Ok(false);
            }
        },
        None => rows.push(Row::new(Mark::Note, "injected", "nothing").note("pass --inject <dll>")),
    }
    ui.rows(&rows);

    if wait {
        ui.line("waiting for the server to exit");
        let status = child.wait().context("waiting for the server")?;
        let _ = std::fs::remove_file(run.join(PID_FILE));
        ui.line(&format!("the server exited with {status}"));
    } else {
        ui.line("run `cargo xtask server logs --follow` to watch it");
    }
    Ok(true)
}

/// Inject a library into a running process.
fn inject_library(pid: u32, library: &Path) -> Result<()> {
    if !library.is_file() {
        bail!("no library at {}", library.display());
    }
    proc::inject(pid, library)
}

// ///////////////////////////////////////////////
// Logs
// ///////////////////////////////////////////////

/// Print the tail of a build's captured output, and optionally keep printing.
fn logs(root: &DevRoot, ui: &Ui, build: Option<&str>, follow: bool, lines: usize) -> Result<bool> {
    let build_id = root.build_id(build)?;
    let run = root.run_dir(&build_id)?;
    let log = run.join(LOG_FILE);
    if !log.is_file() {
        bail!(
            "no log at {}. Run `cargo xtask server run` first.",
            log.display()
        );
    }
    for line in tail_of(&log, lines)? {
        ui.always(&line);
    }
    if !follow {
        return Ok(true);
    }
    let mut offset = std::fs::metadata(&log).map_or(0, |meta| meta.len());
    loop {
        std::thread::sleep(FOLLOW_INTERVAL);
        let Ok(meta) = std::fs::metadata(&log) else {
            continue;
        };
        if meta.len() < offset {
            offset = 0;
        }
        if meta.len() == offset {
            continue;
        }
        let whole = read_text(&log)?;
        let from = usize::try_from(offset).unwrap_or(0).min(whole.len());
        let from = whole.floor_char_boundary(from);
        for line in whole[from..].lines() {
            ui.always(line);
        }
        offset = whole.len() as u64;
    }
}

/// The last `count` lines of a log, as text whatever bytes the server wrote.
fn tail_of(log: &Path, count: usize) -> Result<Vec<String>> {
    let text = read_text(log)?;
    let tail: Vec<&str> = text.lines().rev().take(count).collect();
    Ok(tail.into_iter().rev().map(str::to_string).collect())
}

/// Read a file the server wrote, replacing any byte that is not UTF-8.
///
/// The server's console output is not under the loop's control, and one stray
/// byte must not take the whole log away from the operator.
fn read_text(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

// ///////////////////////////////////////////////
// Stop
// ///////////////////////////////////////////////

/// Ask the server to shut down, and wait for the process to leave.
fn stop(root: &DevRoot, ui: &Ui, build: Option<&str>, timeout: u64, force: bool) -> Result<bool> {
    let build_id = root.build_id(build)?;
    let run = root.run_dir(&build_id)?;
    let exe = root.build_dir(&build_id)?.join(SERVER_EXE);
    ui.section("server stop");
    // The claim below is the one handle held through the wait and the
    // terminate, so the id it checked is the id every later call reaches.
    let server = match recorded_server(&run, &exe)? {
        Recorded::None => {
            ui.row(Mark::Note, "process", "none recorded");
            return Ok(true);
        }
        Recorded::Gone(pid) => {
            ui.row(Mark::Note, "process", &format!("{pid} already gone"));
            return Ok(true);
        }
        Recorded::Stale { pid, image } => {
            ui.rows(&[
                Row::new(Mark::Warn, "process", pid.to_string())
                    .note(format!("is {}, not the server", image.display())),
                Row::new(Mark::Note, PID_FILE, "removed")
                    .note("it was stale, and nothing was signaled"),
            ]);
            return Ok(true);
        }
        Recorded::Ours(server) => server,
    };
    let pid = server.pid();

    // The request is raised in a helper process, and the answer that matters is
    // whether the server left. A helper that reports trouble after the event
    // already landed must not turn a clean stop into a failure.
    let requested = request_stop(pid, ui.quiet());
    let outcome = server.wait(timeout);
    if outcome == Exit::Gone {
        let _ = std::fs::remove_file(run.join(PID_FILE));
        ui.rows(&[
            Row::new(Mark::Ok, "process", pid.to_string()).note("stopped cleanly"),
            Row::new(Mark::Note, "build", &build_id),
        ]);
        return Ok(true);
    }

    if let Err(err) = requested {
        ui.rows(&[Row::new(Mark::Fail, "shutdown request", "not raised").note(format!("{err:#}"))]);
    }
    if !force {
        ui.rows(&[Row::new(Mark::Fail, "process", pid.to_string())
            .note(format!("still running after {timeout}s"))]);
        ui.line("pass --force to terminate it, which risks a half-written save");
        return Ok(false);
    }
    server.terminate()?;
    server.wait(10);
    let _ = std::fs::remove_file(run.join(PID_FILE));
    ui.rows(&[
        Row::new(Mark::Warn, "process", pid.to_string()).note("terminated after the timeout")
    ]);
    Ok(true)
}

/// Raise the shutdown request in a short-lived child process.
///
/// Joining another process's console means leaving this one, which would take
/// the terminal this command is printing to with it. A child does the joining
/// and exits, so the command keeps its console and can report what happened.
/// The child's stderr is a pipe, which survives its `FreeConsole`, so what it
/// says about a refusal reaches this process.
fn request_stop(pid: u32, quiet: bool) -> Result<()> {
    let me = std::env::current_exe().context("finding this executable")?;
    let mut command = Command::new(me);
    if quiet {
        command.arg("--quiet");
    }
    let output = command
        .args(["server", "stop", "--send-break", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .context("asking a helper process to raise the shutdown request")?;
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr);
        bail!("could not ask process {pid} to shut down: {}", said.trim());
    }
    Ok(())
}

// ///////////////////////////////////////////////
// The recorded process
// ///////////////////////////////////////////////

/// What the pid file in a run directory turned out to name.
pub enum Recorded {
    /// No pid file.
    None,
    /// The pid file named a process that is not running. The file is removed.
    Gone(u32),
    /// The pid file named a live process that is not the server. The file is
    /// removed, and nothing is signaled.
    Stale {
        /// The recycled id.
        pid: u32,
        /// What is running under it now.
        image: PathBuf,
    },
    /// The server, held open.
    Ours(proc::Server),
}

/// Read the pid file and find out what it names.
///
/// A pid file that names something other than the server at `exe` is stale and
/// is deleted here, before any caller could signal it.
///
/// # Errors
///
/// Returns an error when the pid file exists and cannot be read.
pub fn recorded_server(run: &Path, exe: &Path) -> Result<Recorded> {
    let path = run.join(PID_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Recorded::None),
        Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
    };
    let Ok(pid) = text.trim().parse::<u32>() else {
        let _ = std::fs::remove_file(&path);
        return Ok(Recorded::None);
    };
    Ok(match proc::claim(pid, exe) {
        Claim::Ours(server) => Recorded::Ours(server),
        Claim::Gone => {
            let _ = std::fs::remove_file(&path);
            Recorded::Gone(pid)
        }
        Claim::Other(image) => {
            let _ = std::fs::remove_file(&path);
            Recorded::Stale { pid, image }
        }
    })
}

// ///////////////////////////////////////////////
// Configuration
// ///////////////////////////////////////////////

/// The configuration the server ships as its default, bound to the loopback
/// address with one slot, with this run's directories and freshly generated
/// user group passwords.
///
/// The server writes this file itself when it is absent, using its own
/// directory defaults relative to the working directory and binding every
/// interface. Writing it here is what keeps saves and logs out of the fetched
/// build and keeps a dev server off the network until someone edits the file.
fn default_config(run: &Path) -> Result<String> {
    let saves = json_string(&run.join("savegame"));
    let logs = json_string(&run.join("logs"));
    let template = include_str!("server_config.json");
    let mut out = template
        .replace("{SAVE_DIRECTORY}", &saves)
        .replace("{LOG_DIRECTORY}", &logs);
    for group in ["ADMIN", "FRIEND", "GUEST", "VISITOR"] {
        out = out.replace(&format!("{{PASSWORD_{group}}}"), &password()?);
    }
    Ok(out)
}

/// A path as a JSON string body, with the separators escaped.
fn json_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "\\\\")
}

/// A password for one user group, drawn from the operating system.
fn password() -> Result<String> {
    const ALPHABET: &[u8] = b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut bytes = [0u8; 20];
    fill_random(&mut bytes)?;
    Ok(bytes
        .iter()
        .map(|byte| ALPHABET[*byte as usize % ALPHABET.len()] as char)
        .collect())
}

/// Fill a buffer from the operating system's random source.
#[cfg(windows)]
fn fill_random(buffer: &mut [u8]) -> Result<()> {
    windows_link::link!("advapi32.dll" "system" fn SystemFunction036(buffer: *mut u8, length: u32) -> u8);
    let length = u32::try_from(buffer.len()).context("the buffer is too large to fill")?;
    // SAFETY: the pointer and the length describe the same buffer, which lives
    // for the call.
    let ok = unsafe { SystemFunction036(buffer.as_mut_ptr(), length) };
    if ok == 0 {
        bail!("the operating system refused to supply random bytes");
    }
    Ok(())
}

/// Fill a buffer from the operating system's random source.
#[cfg(not(windows))]
fn fill_random(buffer: &mut [u8]) -> Result<()> {
    use std::io::Read;
    std::fs::File::open("/dev/urandom")
        .context("opening /dev/urandom")?
        .read_exact(buffer)
        .context("reading /dev/urandom")
}

#[cfg(test)]
mod tests {
    use super::{
        PID_FILE, Recorded, copy_tree, default_config, fetch, json_string, password, read_text,
        recorded_server, refuse_fixture_inside_run, seed, tail_of,
    };
    use crate::root::DevRoot;
    use crate::testutil::TestDir;
    use crate::ui::Ui;
    use std::path::Path;

    /// A library-shaped tree under the sandbox, and a link at `at` pointing
    /// into it. `None` when this machine cannot make links.
    fn library_link(dir: &TestDir, at: &Path) -> Option<std::path::PathBuf> {
        let target = dir
            .path()
            .join("fake")
            .join("SteamLibrary")
            .join("steamapps")
            .join("common")
            .join("Enshrouded")
            .join("linked");
        std::fs::create_dir_all(&target).unwrap();
        if let Some(parent) = at.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        if !TestDir::link_dir(&target, at) {
            eprintln!("skipped: this machine does not allow creating a directory link");
            return None;
        }
        Some(target)
    }

    /// The run directory is where seeding writes and deletes, so a link that
    /// lands it in a library is refused before anything is touched.
    #[test]
    fn seeding_through_a_run_directory_inside_a_library_is_refused() {
        let dir = TestDir::new("seed-link");
        let root = DevRoot::resolve(Some(dir.path())).expect("a sandbox is an ordinary root");
        let Some(target) = library_link(&dir, &root.cache().join("run")) else {
            return;
        };
        let fixture = dir.path().join("fixture");
        dir.write("fixture/world.bin", b"world");
        let ui = Ui::new(true);
        let err = seed(&root, &ui, &fixture, Some("23178631"), true)
            .expect_err("a run directory inside a library is refused");
        assert!(format!("{err}").contains("Steam library"), "{err}");
        assert!(
            std::fs::read_dir(&target).unwrap().next().is_none(),
            "nothing was written through the link"
        );
    }

    /// The staging directory is checked before it is created, so a refusal
    /// leaves nothing behind in the library it would have landed in.
    #[test]
    fn a_refused_fetch_creates_no_staging_directory() {
        let dir = TestDir::new("fetch-link");
        let root = DevRoot::resolve(Some(dir.path())).expect("a sandbox is an ordinary root");
        // An empty file passes the is_file gate and can never run.
        dir.write(".cache/steamcmd/steamcmd.exe", b"");
        let Some(target) = library_link(&dir, &root.server_dir()) else {
            return;
        };
        let ui = Ui::new(true);
        let err =
            fetch(&root, &ui, None).expect_err("a staging directory inside a library is refused");
        assert!(format!("{err}").contains("Steam library"), "{err}");
        assert!(
            !target.join(".fetching").exists(),
            "the refusal created nothing"
        );
    }

    #[test]
    fn a_windows_path_is_escaped_for_json() {
        let cases: &[(&str, &str)] = &[
            (
                r"Z:\repos\ember\.cache\run\23178631",
                r"Z:\\repos\\ember\\.cache\\run\\23178631",
            ),
            ("/home/zach/.cache/run", "/home/zach/.cache/run"),
        ];
        for (input, want) in cases {
            assert_eq!(json_string(Path::new(input)), *want, "path {input}");
        }
    }

    #[test]
    fn the_default_config_carries_this_run_and_no_placeholders() {
        let dir = TestDir::new("config");
        let config = default_config(dir.path()).expect("the config renders");
        assert!(
            !config.contains("{PASSWORD"),
            "every placeholder is replaced"
        );
        assert!(!config.contains("{SAVE_DIRECTORY}"));
        assert!(!config.contains("{LOG_DIRECTORY}"));
        assert!(config.contains(&json_string(&dir.path().join("savegame"))));
        assert!(config.contains(&json_string(&dir.path().join("logs"))));
    }

    /// A dev server that anyone on the network can reach is the wrong default.
    #[test]
    fn the_default_config_binds_the_loopback_with_one_slot() {
        let dir = TestDir::new("config-bind");
        let config = default_config(dir.path()).expect("the config renders");
        // The messages quote the bind lines alone: the rendered config also
        // carries the generated passwords, which a failure must not print.
        let bind: Vec<&str> = config
            .lines()
            .filter(|line| line.contains("\"ip\"") || line.contains("\"slotCount\""))
            .collect();
        assert!(config.contains("\"ip\": \"127.0.0.1\""), "{bind:?}");
        assert!(config.contains("\"slotCount\": 1,"), "{bind:?}");
        assert!(!config.contains("0.0.0.0"), "{bind:?}");
    }

    #[test]
    fn each_user_group_gets_its_own_password() {
        let dir = TestDir::new("config-passwords");
        let config = default_config(dir.path()).expect("the config renders");
        let passwords: Vec<&str> = config
            .lines()
            .filter_map(|line| line.trim().strip_prefix("\"password\": \""))
            .filter_map(|rest| rest.strip_suffix("\","))
            .collect();
        assert_eq!(passwords.len(), 4, "one password per shipped user group");
        for value in &passwords {
            assert_eq!(value.len(), 20, "a generated password is twenty characters");
        }
        let unique: std::collections::BTreeSet<&&str> = passwords.iter().collect();
        assert_eq!(unique.len(), 4, "the four passwords differ");
    }

    #[test]
    fn a_generated_password_uses_an_unambiguous_alphabet() {
        let value = password().expect("the operating system supplies randomness");
        assert_eq!(value.len(), 20);
        for character in value.chars() {
            assert!(
                character.is_ascii_alphanumeric(),
                "unexpected character {character:?}"
            );
            assert!(
                !"lIO01".contains(character),
                "the alphabet leaves out characters that read alike, got {character:?}"
            );
        }
    }

    /// A pid file that names a live process running something other than the
    /// server is stale: the file goes, the answer says what the pid really is,
    /// and nothing is signaled. The test process itself is the live process
    /// that is not the server.
    ///
    /// Only the Windows backend reads a live process's image. The backend on
    /// any other host claims nothing, so the case runs on Windows alone.
    #[cfg(windows)]
    #[test]
    fn a_live_pid_that_is_not_the_server_is_a_stale_file() {
        let dir = TestDir::new("stale-pid");
        let run = dir.path().join("run");
        let exe = dir.write("build/enshrouded_server.exe", b"MZ");
        let pid_file = dir.write("run/server.pid", std::process::id().to_string().as_bytes());
        match recorded_server(&run, &exe).expect("the pid file reads") {
            Recorded::Stale { pid, image } => {
                assert_eq!(pid, std::process::id());
                assert!(
                    image
                        .to_string_lossy()
                        .to_ascii_lowercase()
                        .contains("xtask"),
                    "the image is this test binary, got {}",
                    image.display()
                );
            }
            Recorded::None => panic!("the pid file was there"),
            Recorded::Gone(_) => panic!("this process is alive"),
            Recorded::Ours(_) => panic!("this process is not the server"),
        }
        assert!(!pid_file.exists(), "the stale pid file is removed");
    }

    /// A pid file naming this test process against this test binary is the
    /// server, held open, and the file stays.
    ///
    /// Only the Windows backend holds a live process open, so the case runs
    /// on Windows alone.
    #[cfg(windows)]
    #[test]
    fn a_live_pid_running_the_expected_image_is_ours() {
        let dir = TestDir::new("own-pid");
        let run = dir.path().join("run");
        let me = std::env::current_exe().expect("this binary has a path");
        let pid_file = dir.write("run/server.pid", std::process::id().to_string().as_bytes());
        match recorded_server(&run, &me).expect("the pid file reads") {
            Recorded::Ours(server) => assert_eq!(server.pid(), std::process::id()),
            _ => panic!("this process is running the expected image"),
        }
        assert!(pid_file.exists(), "a live server's pid file stays");
    }

    #[test]
    fn a_pid_file_that_does_not_parse_is_removed_and_reads_as_none() {
        let dir = TestDir::new("bad-pid");
        let run = dir.path().join("run");
        let exe = dir.path().join("build").join("enshrouded_server.exe");
        let pid_file = dir.write("run/server.pid", b"not a number");
        assert!(matches!(
            recorded_server(&run, &exe).unwrap(),
            Recorded::None
        ));
        assert!(!pid_file.exists());
        assert!(matches!(
            recorded_server(&run, &exe).unwrap(),
            Recorded::None
        ));
        let _ = PID_FILE;
    }

    /// Seeding removes and rewrites the run directory's save, so a fixture in
    /// there would be deleted or copied into its own child.
    #[test]
    fn a_fixture_inside_the_run_directory_is_refused() {
        let dir = TestDir::new("fixture-inside");
        let run = dir.path().join("run");
        dir.write("run/savegame/world.bin", b"world");
        let cases = [
            run.clone(),
            run.join("savegame"),
            run.join("..").join("run"),
        ];
        for fixture in cases {
            let err = refuse_fixture_inside_run(&fixture, &run).expect_err(&format!(
                "{} is inside the run directory",
                fixture.display()
            ));
            assert!(
                format!("{err}").contains("inside the run directory"),
                "{err}"
            );
        }
        refuse_fixture_inside_run(&dir.path().join("elsewhere"), &run)
            .expect("a directory beside the run directory is fine");
    }

    /// A copy never descends into the directory it is writing.
    #[test]
    fn copying_a_tree_into_its_own_child_stops_at_the_child() {
        let dir = TestDir::new("copy-self");
        dir.write("tree/a.txt", b"a");
        dir.write("tree/sub/b.txt", b"b");
        let tree = dir.path().join("tree");
        let into = tree.join("copy");
        let files = copy_tree(&tree, &into).expect("the copy finishes");
        assert_eq!(files, 2);
        assert!(into.join("a.txt").is_file());
        assert!(into.join("sub").join("b.txt").is_file());
        assert!(
            !into.join("copy").exists(),
            "the destination is not copied into itself"
        );
    }

    /// One byte that is not UTF-8 must not take the whole log away.
    #[test]
    fn a_log_with_bytes_that_are_not_utf8_still_reads() {
        let dir = TestDir::new("log-bytes");
        let log = dir.write("server.log", b"[Info] \xC4g\nok\nlast\n");
        let text = read_text(&log).expect("the log reads");
        assert!(text.contains("ok"), "{text}");
        assert_eq!(tail_of(&log, 2).unwrap(), vec!["ok", "last"]);
    }
}
