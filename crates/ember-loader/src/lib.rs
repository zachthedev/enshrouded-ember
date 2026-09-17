//! The library the server loads.
//!
//! On Windows it ships as a proxy: a library named for one the server already
//! imports, placed beside the executable, forwarding every export to the real
//! system copy while starting Ember on the side. `POWRPROF.dll` is the default,
//! because the server imports it, it exports two functions, and neither other
//! public Enshrouded loader claims it. The same binary renamed to
//! `IPHLPAPI.dll`, `WINMM.dll` or `dbghelp.dll` forwards those instead.
//!
//! Wine and Proton ship builtin copies of all four and prefer them, so a host on
//! Linux sets `WINEDLLOVERRIDES="powrprof=n,b"`.
//!
//! Development and continuous integration inject this same library with
//! `dll-syringe`, which leaves the fetched server directory untouched.
