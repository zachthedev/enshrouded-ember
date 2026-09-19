//! Starting, identifying, watching and stopping the server process.
//!
//! A process id on its own names nothing. Windows hands a freed id out again as
//! soon as its last handle closes, so a pid file can name the operator's game
//! client an hour after the server it was written for exited. Every action here
//! therefore starts with [`claim`], which opens the pid, reads the executable
//! behind it and compares that to the server the run directory belongs to. The
//! handle it returns is held through the last action, and an open handle is
//! what keeps an id from being recycled.
//!
//! The clean stop is a console control event, because the dedicated server has
//! no remote console and answers Ctrl-Break by saving and exiting. A hard
//! terminate is behind `--force`, since killing a server mid-save is how a
//! friend server loses a world.
//!
//! Injection is the remote-thread load: the library path is written into the
//! target and a thread is started on `LoadLibraryW`. Both processes are x64 and
//! `kernel32.dll` sits at one base per boot, so the address this process reads
//! is the address the target calls. The load is confirmed from the target's
//! module list, never from the thread's exit code, because that code is 32 bits
//! of a 64-bit handle and becomes the process exit status if the load crashes.
//!
//! No crate covers any of these operations on a stable toolchain, so the calls
//! are declared here with `windows_link::link!`, which is the same way the rest
//! of this project declares the twenty or so classic Win32 exports it needs.

use std::path::{Path, PathBuf};
use std::process::Child;

use anyhow::Result;

/// `WaitForSingleObject` saying the object signaled.
#[cfg(test)]
const WAIT_OBJECT_0: u32 = 0;

/// `WaitForSingleObject` saying the wait ran out.
#[cfg(any(windows, test))]
const WAIT_TIMEOUT: u32 = 258;

/// What a wait for exit concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// The process left on its own.
    Gone,
    /// The process is still running.
    #[cfg_attr(
        not(windows),
        expect(dead_code, reason = "only the Windows backend waits on a live process")
    )]
    Running,
}

/// What a recorded pid turned out to name.
pub enum Claim {
    /// Nothing alive holds the id.
    Gone,
    /// A live process holds the id and it is not the server. The path is the
    /// executable it is running.
    #[cfg_attr(
        not(windows),
        expect(dead_code, reason = "only the Windows backend opens a live process")
    )]
    Other(PathBuf),
    /// The server, held open.
    #[cfg_attr(
        not(windows),
        expect(dead_code, reason = "only the Windows backend opens a live process")
    )]
    Ours(Server),
}

/// What a remote library load came to.
#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadOutcome {
    /// The library is in the target's module list.
    Loaded,
    /// The loader thread did not finish inside the wait.
    TimedOut,
    /// The target exited while loading, with this status.
    ProcessExited(u32),
    /// The target is alive, the library is not in its module list, and the
    /// loader thread's exit code could not be read.
    ThreadCodeUnreadable,
    /// The target is alive and the library is not in its module list.
    Refused {
        /// The loader thread's exit code, as a hint.
        thread_code: u32,
    },
}

/// Judge a remote library load from the facts the calls establish.
///
/// The order is the order of evidence. A wait that ran out says nothing about
/// the load. A target that exited took the answer with it, and its exit status
/// is what to report, whatever the thread's code says. A library in the module
/// list is loaded, whatever the thread's code says, because that code is the low
/// half of a 64-bit handle and can be zero. Only then does the thread code
/// matter, and only as a hint.
#[cfg(any(windows, test))]
pub fn judge_load(
    thread_wait: u32,
    process_alive: bool,
    process_status: u32,
    thread_code: Option<u32>,
    in_module_list: bool,
) -> LoadOutcome {
    if thread_wait == WAIT_TIMEOUT {
        return LoadOutcome::TimedOut;
    }
    if !process_alive {
        return LoadOutcome::ProcessExited(process_status);
    }
    if in_module_list {
        return LoadOutcome::Loaded;
    }
    match thread_code {
        None => LoadOutcome::ThreadCodeUnreadable,
        Some(thread_code) => LoadOutcome::Refused { thread_code },
    }
}

/// Whether two executable paths name the same file.
///
/// Both sides canonicalize when they exist, which folds case, separators and
/// links. When either does not exist the comparison falls back to the text,
/// case folded, which is what Windows does with a path it cannot open.
#[cfg(any(windows, test))]
pub fn same_executable(left: &Path, right: &Path) -> bool {
    if let (Ok(a), Ok(b)) = (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        return a == b;
    }
    let fold = |path: &Path| path.to_string_lossy().to_ascii_lowercase();
    fold(left) == fold(right)
}

#[cfg(windows)]
mod platform {
    use std::os::windows::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};

    use anyhow::{Context, Result, bail};

    use super::{Claim, Exit, LoadOutcome, WAIT_TIMEOUT, judge_load, same_executable};

    /// Put the child in its own process group so a control event can be aimed
    /// at it without reaching every process on the console. The group id of
    /// such a child is its own process id, which is what the stop aims at.
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

    /// Give the child a console of its own, hidden. Closing the operator's
    /// terminal then leaves the server running, and `AttachConsole` still
    /// reaches it because it still has a console.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    /// Right to read a process's exit code and image name.
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

    /// Right to wait on a process handle.
    const SYNCHRONIZE: u32 = 0x0010_0000;

    /// Right to kill a process.
    const PROCESS_TERMINATE: u32 = 0x0001;

    /// The control event a console application sees as a request to shut down.
    const CTRL_BREAK_EVENT: u32 = 1;

    /// The exit code a process reports while it is still running.
    const STILL_ACTIVE: u32 = 259;

    /// Longest path `QueryFullProcessImageNameW` is asked for.
    const PATH_CAPACITY: usize = 32_768;

    /// `K32EnumProcessModulesEx` filter taking every module, whatever its width.
    const LIST_MODULES_ALL: u32 = 0x03;

    /// Most modules the injection check reads back.
    const MODULE_CAPACITY: usize = 1024;

    windows_link::link!("kernel32.dll" "system" fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize);
    windows_link::link!("kernel32.dll" "system" fn CloseHandle(handle: isize) -> i32);
    windows_link::link!("kernel32.dll" "system" fn WaitForSingleObject(handle: isize, milliseconds: u32) -> u32);
    windows_link::link!("kernel32.dll" "system" fn GetExitCodeProcess(handle: isize, code: *mut u32) -> i32);
    windows_link::link!("kernel32.dll" "system" fn TerminateProcess(handle: isize, code: u32) -> i32);
    windows_link::link!("kernel32.dll" "system" fn GenerateConsoleCtrlEvent(event: u32, group: u32) -> i32);
    windows_link::link!("kernel32.dll" "system" fn AttachConsole(pid: u32) -> i32);
    windows_link::link!("kernel32.dll" "system" fn FreeConsole() -> i32);
    windows_link::link!("kernel32.dll" "system" fn SetConsoleCtrlHandler(handler: Option<unsafe extern "system" fn(u32) -> i32>, add: i32) -> i32);
    windows_link::link!("kernel32.dll" "system" fn QueryFullProcessImageNameW(process: isize, flags: u32, name: *mut u16, size: *mut u32) -> i32);
    windows_link::link!("kernel32.dll" "system" fn K32EnumProcessModulesEx(process: isize, modules: *mut isize, size: u32, needed: *mut u32, filter: u32) -> i32);
    windows_link::link!("kernel32.dll" "system" fn K32GetModuleFileNameExW(process: isize, module: isize, name: *mut u16, size: u32) -> u32);

    /// A process handle that closes itself.
    struct Handle(isize);

    impl Handle {
        /// Open a process by id, or report that it is gone.
        fn open(pid: u32, access: u32) -> Option<Self> {
            // SAFETY: OpenProcess takes plain integers and returns a handle or
            // zero. Nothing is dereferenced.
            let raw = unsafe { OpenProcess(access, 0, pid) };
            (raw != 0).then_some(Self(raw))
        }

        /// The exit code, or `None` while the process runs.
        fn exit_code(&self) -> Option<u32> {
            let mut code: u32 = 0;
            // SAFETY: the handle is live and `code` is a valid writable u32.
            let ok = unsafe { GetExitCodeProcess(self.0, &raw mut code) };
            (ok != 0 && code != STILL_ACTIVE).then_some(code)
        }

        /// Whether the process is still running, by a zero-length wait.
        fn alive(&self) -> bool {
            // SAFETY: the handle is live for the call.
            unsafe { WaitForSingleObject(self.0, 0) == WAIT_TIMEOUT }
        }

        /// The path of the executable the process is running.
        fn image(&self) -> Option<PathBuf> {
            let mut buffer = vec![0u16; PATH_CAPACITY];
            let mut length = u32::try_from(buffer.len()).ok()?;
            // SAFETY: the buffer is live and `length` carries its capacity in
            // characters, which the call updates to the characters written.
            let ok = unsafe {
                QueryFullProcessImageNameW(self.0, 0, buffer.as_mut_ptr(), &raw mut length)
            };
            if ok == 0 {
                return None;
            }
            let length = usize::try_from(length).ok()?;
            Some(PathBuf::from(String::from_utf16_lossy(&buffer[..length])))
        }

        /// Every module the process has loaded, by file path.
        fn modules(&self) -> Vec<PathBuf> {
            let mut handles = vec![0isize; MODULE_CAPACITY];
            let capacity = u32::try_from(handles.len() * size_of::<isize>()).unwrap_or(u32::MAX);
            let mut needed: u32 = 0;
            // SAFETY: the array is live and `capacity` is its size in bytes.
            let ok = unsafe {
                K32EnumProcessModulesEx(
                    self.0,
                    handles.as_mut_ptr(),
                    capacity,
                    &raw mut needed,
                    LIST_MODULES_ALL,
                )
            };
            if ok == 0 {
                return Vec::new();
            }
            let count = (needed as usize / size_of::<isize>()).min(handles.len());
            let mut out = Vec::with_capacity(count);
            for module in &handles[..count] {
                let mut name = vec![0u16; PATH_CAPACITY];
                let size = u32::try_from(name.len()).unwrap_or(u32::MAX);
                // SAFETY: the buffer is live and `size` is its capacity.
                let written =
                    unsafe { K32GetModuleFileNameExW(self.0, *module, name.as_mut_ptr(), size) };
                let written = usize::try_from(written).unwrap_or(0);
                if written > 0 {
                    out.push(PathBuf::from(String::from_utf16_lossy(&name[..written])));
                }
            }
            out
        }
    }

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: the handle came from OpenProcess and is closed once.
            unsafe { CloseHandle(self.0) };
        }
    }

    /// The server, held open so its id cannot be recycled while it is acted on.
    pub struct Server {
        handle: Handle,
        pid: u32,
    }

    impl Server {
        /// The process id.
        pub fn pid(&self) -> u32 {
            self.pid
        }

        /// Wait up to `seconds` for the process to exit.
        pub fn wait(&self, seconds: u64) -> Exit {
            let milliseconds = u32::try_from(seconds.saturating_mul(1000)).unwrap_or(u32::MAX);
            // SAFETY: the handle is live for the length of the wait.
            let result = unsafe { WaitForSingleObject(self.handle.0, milliseconds) };
            if result == WAIT_TIMEOUT {
                Exit::Running
            } else {
                Exit::Gone
            }
        }

        /// Kill the process outright.
        pub fn terminate(&self) -> Result<()> {
            // The handle this holds pins the id, so a second handle opened on
            // the same id names the same process.
            let Some(killer) = Handle::open(self.pid, PROCESS_TERMINATE) else {
                bail!("could not open process {} to terminate it", self.pid);
            };
            // SAFETY: the handle carries PROCESS_TERMINATE and is live.
            if unsafe { TerminateProcess(killer.0, 1) } == 0 {
                bail!("could not terminate process {}", self.pid);
            }
            Ok(())
        }
    }

    /// Open a pid and say whether it is the server at `expected`.
    pub fn claim(pid: u32, expected: &Path) -> Claim {
        let Some(handle) = Handle::open(pid, SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION)
        else {
            return Claim::Gone;
        };
        if handle.exit_code().is_some() {
            return Claim::Gone;
        }
        let Some(image) = handle.image() else {
            return Claim::Other(PathBuf::from("an executable whose path could not be read"));
        };
        if same_executable(&image, expected) {
            Claim::Ours(Server { handle, pid })
        } else {
            Claim::Other(image)
        }
    }

    /// Start the server, with its output going to a log file.
    pub fn spawn(exe: &Path, args: &[String], working_dir: &Path, log: &Path) -> Result<Child> {
        let file =
            std::fs::File::create(log).with_context(|| format!("creating {}", log.display()))?;
        let errors = file
            .try_clone()
            .with_context(|| format!("opening a second handle on {}", log.display()))?;
        Command::new(exe)
            .args(args)
            .current_dir(working_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::from(file))
            .stderr(Stdio::from(errors))
            .creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW)
            .spawn()
            .with_context(|| format!("launching {}", exe.display()))
    }

    /// Swallow a control event raised on this console.
    ///
    /// A null handler covers Ctrl-C alone, so a Ctrl-Break that reached this
    /// process would end it before it could report. Returning true says the
    /// event is handled.
    unsafe extern "system" fn swallow_control_event(_event: u32) -> i32 {
        1
    }

    /// Ask a process to shut down, from a process that shares no console.
    ///
    /// The caller has to leave its own console before joining the target's,
    /// and has to stop reacting to the event it is about to raise. Both are
    /// why this runs in a short-lived child process rather than in the command
    /// the operator typed.
    pub fn send_break(pid: u32) -> Result<()> {
        // SAFETY: each call takes plain integers or a function pointer with the
        // signature the handler list expects.
        unsafe {
            FreeConsole();
            if AttachConsole(pid) == 0 {
                bail!("could not join the console of process {pid}; it is not a console process");
            }
            SetConsoleCtrlHandler(Some(swallow_control_event), 1);
            // The group id is the server's own process id, because the server
            // was started in a process group of its own. Group zero would send
            // the event to every process on the console, which includes the
            // command that asked for the stop.
            if GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid) == 0 {
                bail!("could not raise a console control event for process {pid}");
            }
        }
        Ok(())
    }

    /// Rights a remote-thread load needs on the target.
    const INJECTION_ACCESS: u32 = 0x0002 | 0x0400 | 0x0008 | 0x0020 | 0x0010 | SYNCHRONIZE;

    /// Reserve and commit pages in the target.
    const MEM_COMMIT_RESERVE: u32 = 0x1000 | 0x2000;

    /// Give the pages back.
    const MEM_RELEASE: u32 = 0x8000;

    /// The protection a path buffer needs.
    const PAGE_READWRITE: u32 = 0x04;

    /// Milliseconds to wait for the remote loader thread.
    const LOAD_TIMEOUT_MS: u32 = 30_000;

    windows_link::link!("kernel32.dll" "system" fn VirtualAllocEx(process: isize, address: *mut core::ffi::c_void, size: usize, kind: u32, protect: u32) -> *mut core::ffi::c_void);
    windows_link::link!("kernel32.dll" "system" fn VirtualFreeEx(process: isize, address: *mut core::ffi::c_void, size: usize, kind: u32) -> i32);
    windows_link::link!("kernel32.dll" "system" fn WriteProcessMemory(process: isize, address: *mut core::ffi::c_void, buffer: *const core::ffi::c_void, size: usize, written: *mut usize) -> i32);
    windows_link::link!("kernel32.dll" "system" fn GetModuleHandleW(name: *const u16) -> isize);
    windows_link::link!("kernel32.dll" "system" fn GetProcAddress(module: isize, name: *const u8) -> *const core::ffi::c_void);
    windows_link::link!("kernel32.dll" "system" fn CreateRemoteThread(process: isize, attributes: *mut core::ffi::c_void, stack: usize, start: *const core::ffi::c_void, parameter: *mut core::ffi::c_void, flags: u32, thread_id: *mut u32) -> isize);
    windows_link::link!("kernel32.dll" "system" fn GetExitCodeThread(thread: isize, code: *mut u32) -> i32);

    /// A page range in another process, freed when it drops.
    struct RemoteBuffer {
        process: isize,
        address: *mut core::ffi::c_void,
    }

    impl Drop for RemoteBuffer {
        fn drop(&mut self) {
            // SAFETY: the address came from VirtualAllocEx on this process
            // handle, and MEM_RELEASE takes a zero size.
            unsafe { VirtualFreeEx(self.process, self.address, 0, MEM_RELEASE) };
        }
    }

    /// Load a library into another process on a thread of its own.
    pub fn inject(pid: u32, library: &Path) -> Result<()> {
        use std::os::windows::ffi::OsStrExt;

        let absolute = std::path::absolute(library)
            .with_context(|| format!("resolving {}", library.display()))?;
        let wide: Vec<u16> = absolute
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let bytes = wide.len() * 2;

        let Some(handle) = Handle::open(pid, INJECTION_ACCESS) else {
            bail!("could not open process {pid} for injection");
        };

        // SAFETY: every argument is a plain integer or a null pointer, and the
        // returned address is checked before it is used.
        let address = unsafe {
            VirtualAllocEx(
                handle.0,
                std::ptr::null_mut(),
                bytes,
                MEM_COMMIT_RESERVE,
                PAGE_READWRITE,
            )
        };
        if address.is_null() {
            bail!("could not reserve {bytes} bytes in process {pid}");
        }
        let buffer = RemoteBuffer {
            process: handle.0,
            address,
        };

        // SAFETY: the destination was just reserved with exactly this size, and
        // the source is the local buffer that is still alive.
        let written = unsafe {
            WriteProcessMemory(
                handle.0,
                buffer.address,
                wide.as_ptr().cast(),
                bytes,
                std::ptr::null_mut(),
            )
        };
        if written == 0 {
            bail!("could not write the library path into process {pid}");
        }

        let module_name: Vec<u16> = "kernel32.dll"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: both strings are NUL terminated and live across the calls.
        let loader = unsafe {
            let module = GetModuleHandleW(module_name.as_ptr());
            if module == 0 {
                bail!("kernel32.dll is not loaded in this process");
            }
            GetProcAddress(module, c"LoadLibraryW".as_ptr().cast())
        };
        if loader.is_null() {
            bail!("kernel32.dll exports no LoadLibraryW");
        }

        // SAFETY: the start address is an exported function of a module every
        // process shares, and the parameter is the buffer just written.
        let thread = unsafe {
            CreateRemoteThread(
                handle.0,
                std::ptr::null_mut(),
                0,
                loader,
                buffer.address,
                0,
                std::ptr::null_mut(),
            )
        };
        if thread == 0 {
            bail!("could not start a loader thread in process {pid}");
        }
        let thread = Handle(thread);

        // SAFETY: the thread handle is live for the wait and the read.
        let (waited, thread_code) = unsafe {
            let waited = WaitForSingleObject(thread.0, LOAD_TIMEOUT_MS);
            let mut code: u32 = 0;
            let readable = GetExitCodeThread(thread.0, &raw mut code) != 0;
            (waited, readable.then_some(code))
        };
        if waited == WAIT_TIMEOUT {
            // The loader may still be reading the path. Leaking one page in a
            // process the operator is about to stop costs nothing; freeing it
            // under a running loader is a use after free in the target.
            std::mem::forget(buffer);
            bail!(
                "the loader thread in process {pid} did not finish in {} seconds. The load may still complete.",
                LOAD_TIMEOUT_MS / 1000
            );
        }
        let alive = handle.alive();
        let status = handle.exit_code().unwrap_or(0);
        let in_module_list = alive
            && handle
                .modules()
                .iter()
                .any(|module| same_executable(module, &absolute));
        report_load(
            pid,
            &absolute,
            judge_load(waited, alive, status, thread_code, in_module_list),
        )
    }

    /// Turn a load verdict into the caller's result.
    fn report_load(pid: u32, library: &Path, outcome: LoadOutcome) -> Result<()> {
        match outcome {
            LoadOutcome::Loaded => Ok(()),
            LoadOutcome::TimedOut => bail!(
                "the loader thread in process {pid} did not finish. The load may still complete."
            ),
            LoadOutcome::ProcessExited(status) => bail!(
                "process {pid} exited with status {status:#010x} while loading {}",
                library.display()
            ),
            LoadOutcome::ThreadCodeUnreadable => bail!(
                "process {pid} did not load {} and the loader thread's exit code could not be read",
                library.display()
            ),
            LoadOutcome::Refused { thread_code } => bail!(
                "process {pid} did not load {}; the loader thread returned {thread_code:#x}.                  The library's own dependencies are the usual cause.",
                library.display()
            ),
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use std::path::Path;
    use std::process::Child;

    use anyhow::{Result, bail};

    use super::{Claim, Exit};

    /// The message every call on a host with no Windows server carries.
    const UNSUPPORTED: &str = "Keen ships a Windows dedicated server only, so the dev loop drives the server through \
         Windows process control. On Linux, run this loop inside Wine or Proton.";

    /// A server handle on a host that cannot hold one.
    pub struct Server {
        pid: u32,
    }

    impl Server {
        /// The process id.
        pub fn pid(&self) -> u32 {
            self.pid
        }

        /// Report gone, because nothing was started.
        #[expect(
            clippy::unused_self,
            reason = "the signature matches the Windows backend"
        )]
        pub fn wait(&self, _seconds: u64) -> Exit {
            Exit::Gone
        }

        /// Refuse to terminate, because nothing was started.
        #[expect(
            clippy::unused_self,
            reason = "the signature matches the Windows backend"
        )]
        pub fn terminate(&self) -> Result<()> {
            bail!("{UNSUPPORTED}")
        }
    }

    /// Report gone, because nothing was started.
    pub fn claim(_pid: u32, _expected: &Path) -> Claim {
        Claim::Gone
    }

    /// Refuse to launch, because there is no server binary for this host.
    pub fn spawn(_exe: &Path, _args: &[String], _dir: &Path, _log: &Path) -> Result<Child> {
        bail!("{UNSUPPORTED}")
    }

    /// Refuse to signal, because nothing was started.
    pub fn send_break(_pid: u32) -> Result<()> {
        bail!("{UNSUPPORTED}")
    }

    /// Refuse to inject, because there is no process to inject into.
    pub fn inject(_pid: u32, _library: &Path) -> Result<()> {
        bail!("{UNSUPPORTED}")
    }
}

pub use platform::Server;

/// Open a recorded pid and say whether it is the server at `expected`.
///
/// The handle inside [`Claim::Ours`] pins the id for as long as it lives, so
/// every later action on it reaches the process that was checked.
pub fn claim(pid: u32, expected: &Path) -> Claim {
    platform::claim(pid, expected)
}

/// Start the server with its output going to a log file.
///
/// The returned child is the one handle that pins the new id. The caller keeps
/// it alive through everything it does with the pid.
///
/// # Errors
///
/// Returns an error when the log cannot be created or the process cannot start.
pub fn spawn(exe: &Path, args: &[String], working_dir: &Path, log: &Path) -> Result<Child> {
    platform::spawn(exe, args, working_dir, log)
}

/// Ask a process to shut down cleanly.
///
/// # Errors
///
/// Returns an error when the control event cannot be raised.
pub fn send_break(pid: u32) -> Result<()> {
    platform::send_break(pid)
}

/// Load a library into a running process.
///
/// # Errors
///
/// Returns an error when the process refuses to open, the path cannot be
/// written into it, the target exits during the load, or the library does not
/// appear in the target's module list afterwards.
pub fn inject(pid: u32, library: &Path) -> Result<()> {
    platform::inject(pid, library)
}

#[cfg(test)]
mod tests {
    use super::{LoadOutcome, WAIT_OBJECT_0, WAIT_TIMEOUT, judge_load, same_executable};
    use crate::testutil::TestDir;

    /// One set of facts and the verdict they must produce.
    type Case = (&'static str, u32, bool, u32, Option<u32>, bool, LoadOutcome);

    #[test]
    fn the_load_verdict_follows_the_evidence_in_order() {
        let cases: &[Case] = &[
            (
                "a wait that ran out says nothing",
                WAIT_TIMEOUT,
                true,
                0,
                Some(1),
                false,
                LoadOutcome::TimedOut,
            ),
            (
                "a crash in DllMain ends the process and its status is the answer",
                WAIT_OBJECT_0,
                false,
                0xC000_0005,
                Some(0xC000_0005),
                false,
                LoadOutcome::ProcessExited(0xC000_0005),
            ),
            (
                "a module whose handle has zero low bits is still loaded",
                WAIT_OBJECT_0,
                true,
                0,
                Some(0),
                true,
                LoadOutcome::Loaded,
            ),
            (
                "the module list decides even with no thread code",
                WAIT_OBJECT_0,
                true,
                0,
                None,
                true,
                LoadOutcome::Loaded,
            ),
            (
                "no module and no code is its own answer",
                WAIT_OBJECT_0,
                true,
                0,
                None,
                false,
                LoadOutcome::ThreadCodeUnreadable,
            ),
            (
                "a zero code with no module is a refusal",
                WAIT_OBJECT_0,
                true,
                0,
                Some(0),
                false,
                LoadOutcome::Refused { thread_code: 0 },
            ),
            (
                "a nonzero code with no module is still a refusal",
                WAIT_OBJECT_0,
                true,
                0,
                Some(0x7FF8_0000),
                false,
                LoadOutcome::Refused {
                    thread_code: 0x7FF8_0000,
                },
            ),
        ];
        for (label, wait, alive, status, code, listed, want) in cases {
            assert_eq!(
                judge_load(*wait, *alive, *status, *code, *listed),
                *want,
                "{label}"
            );
        }
    }

    #[test]
    fn executables_compare_by_the_file_they_name() {
        let dir = TestDir::new("same-exe");
        let real = dir.write("build/enshrouded_server.exe", b"MZ");
        let via_dots = dir
            .path()
            .join("build")
            .join("..")
            .join("build")
            .join("enshrouded_server.exe");
        assert!(
            same_executable(&real, &via_dots),
            "a dotted path names the same file"
        );
        let upper = dir.path().join("build").join("ENSHROUDED_SERVER.EXE");
        assert!(
            same_executable(&real, &upper),
            "case does not separate two files on Windows"
        );
        let other = dir.write("build/enshrouded.exe", b"MZ");
        assert!(
            !same_executable(&real, &other),
            "a different file is different"
        );
        let missing = dir.path().join("build").join("absent.exe");
        assert!(
            !same_executable(&real, &missing),
            "a file that is not there is not the server"
        );
    }
}
