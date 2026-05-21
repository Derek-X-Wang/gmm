//! Runtime-only types that live in the Tauri layer (never on the Core).
//!
//! Anything in here is not unit-tested via `cargo test` directly — Tauri
//! commands are the only consumers and they're exercised by the Windows
//! smoke (`cargo xtask test-session`) and manual `pnpm tauri dev` runs.

pub mod session;

pub use session::{SessionRuntime, SESSION_ENDED_EVENT, SESSION_STARTED_EVENT};
