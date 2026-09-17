//! Development-only library for driving a test server.
//!
//! The real client is a Vulkan application with no headless mode and a Steam
//! login per machine, and the wire protocol rides Steam networking, so neither
//! drives a test. The server binary registers `ecs.FakePlayerSpawner`,
//! `ecs.PlayerTestInput` and a set of test entity components, so tests drive the
//! real server from inside its own process instead.
//!
//! This library opens a channel on the loopback interface and answers commands:
//! spawn a player, place an entity, read state back, step the simulation. Tests
//! call it; servers never load it. It is never published.
