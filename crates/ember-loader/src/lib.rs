//! The library the server loads.
//!
//! On Windows it ships as a proxy: a library named for one the server already
//! imports, placed beside the executable, forwarding its exports to the real
//! system copy while starting Ember on the side.
//!
//! The server imports 13 libraries and 344 functions. Three of those libraries
//! sit outside the `KnownDLLs` list and can therefore be proxied: `POWRPROF.dll`
//! (139 exports, all named), `IPHLPAPI.DLL` (313 exports, all named) and
//! `dbghelp.dll` (268 exports, 16 of them ordinal only). `POWRPROF.dll` is the
//! default, because no other public Enshrouded loader claims it and its exports
//! are all named.
//!
//! Forwarding covers every export of the library being stood in for, not only
//! the handful the server imports, because anything else in the process may
//! import the rest.
//!
//! Wine and Proton ship builtin copies of all three and prefer them, so a host
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
//! land there. `cargo xtask server fetch --validate` restores everything else.
