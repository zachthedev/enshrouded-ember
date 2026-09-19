//! The library the server loads.
//!
//! On Windows it ships as a proxy: a library named for one the server already
//! imports, placed beside the executable, forwarding its exports to the real
//! system copy while starting Ember on the side.
//!
//! Of the libraries the server imports, `POWRPROF.dll`, `IPHLPAPI.DLL` and
//! `dbghelp.dll` sit outside the `KnownDLLs` list and can therefore be proxied.
//! `POWRPROF.dll` is the default, because no other public Enshrouded loader
//! claims it and its exports are all named. `dbghelp.dll` exports some
//! functions by ordinal only.
//!
//! Forwarding covers every export of the library being stood in for, not only
//! the handful the server imports, because anything else in the process may
//! import the rest.
//!
//! Wine and Proton ship builtin copies of each and prefer them, so a host
//! on Linux sets `WINEDLLOVERRIDES="powrprof=n,b"`.
//!
//! Development and continuous integration load this same library into a running
//! server instead, by writing its path into the process and starting a thread on
//! `LoadLibraryW`, so no copy of it is placed beside the executable.
//!
//! A run still leaves files in the build directory. The server's own
//! `saveDirectory` and `logDirectory` point at the run directory beside it, but
//! the Steamworks client library resolves its `logs` and `config` directories
//! against the executable rather than the working directory, so its own files
//! land there. Deleting the build directory and running
//! `cargo xtask server fetch` again restores everything else.

pub mod discovery;
pub mod entry;
pub mod host_api;
pub mod logging;
pub mod proxy;
pub mod report;
pub mod startup;
