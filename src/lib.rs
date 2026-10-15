//! bonsai — grandfather-father-son (GFS) snapshot manager for bcachefs.
//!
//! The crate is split so the retention logic is pure and testable:
//!
//! * [`config`] — TOML configuration model.
//! * [`snapshot`] — snapshot naming/parsing (`<tier>-YYYYMMDD`).
//! * [`policy`] — pure GFS planner: what to create and what to prune.
//! * [`bcachefs`] — side effects against the `bcachefs` CLI behind a trait.
//! * [`app`] — wires the pieces together for the CLI.
//! * [`lock`] — exclusive non-blocking lock to prevent overlapping runs.
//! * [`logger`] — tracing subscriber setup.

pub mod app;
pub mod bcachefs;
pub mod config;
pub mod lock;
pub mod logger;
pub mod policy;
pub mod snapshot;

pub use config::Config;
pub use policy::{Plan, plan};
pub use snapshot::{Snapshot, Tier};
