//! lazierdocker: a mouse-first terminal UI for Docker.
//!
//! # Layout
//!
//! - [`app`]: the application. `App` holds all state; input handlers
//!   (`input`, `overlay`, `find`, `copy_mode`) change it and return `Effect`s;
//!   `run` owns the terminal and the event loop and executes effects.
//!   `inspect` decides what each pane shows (`sync_panes`), `panes` arranges
//!   panes, and `render` draws a frame and records the hit map that the
//!   next mouse event is tested against.
//! - [`docker`]: the daemon. `client` connects and lists everything into a
//!   `model::Snapshot`; `refresh` re-lists on docker events; `ops` runs
//!   actions; `stats` streams per-container stats; `present` formats rows.
//! - [`ui`]: drawing building blocks with no app state: the sidebar tree,
//!   the split layout tree, pane borders, the status bar, text helpers.
//! - [`doc`]: read-only documents (inspect output, env, top, stats graphs).
//! - [`term`]: embedded terminals (libghostty-vt): shells, attach, logs.
//! - [`event`]: messages from background tasks to the event loop.
//!
//! # Flow
//!
//! Background tasks (snapshot refresh, stats, log streams, terminal output,
//! loads) send [`event::AppEvent`]s. After every event, the loop calls
//! `App::sync_panes`, executes the resulting effects, and redraws when
//! `App::dirty` is set (at most about 60 times a second).
#![cfg_attr(test, allow(clippy::field_reassign_with_default))]

pub mod app;
pub mod clipboard;
pub mod doc;
pub mod docker;
pub mod event;
pub mod term;
pub mod ui;
