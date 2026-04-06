//! Arbiter MCP Server — Coding Agent Policy Engine.
//!
//! Library crate exposing server components for integration testing.
//! The binary entry point is in `main.rs`.

// Database (rusqlite) is Send but not Sync; Arc is used for shared
// ownership in the hot-reload model, not for cross-thread access.
#![allow(clippy::arc_with_non_send_sync)]

pub mod agents;
pub mod config;
pub mod db;
pub mod features;
pub mod metrics;
pub mod server;
pub mod tools;
