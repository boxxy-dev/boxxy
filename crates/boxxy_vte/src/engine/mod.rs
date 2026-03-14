//! Boxxy Terminal Engine.

#![warn(rust_2018_idioms)]
#![deny(clippy::all, clippy::if_not_else, clippy::enum_glob_use)]
#![cfg_attr(clippy, deny(warnings))]

pub mod event;
pub mod event_loop;
pub mod grid;
pub mod index;
pub mod selection;
pub mod sync;
pub mod term;
pub mod thread;
pub mod tty;
pub mod vi_mode;
pub mod vte;
pub mod kitty;

pub use crate::engine::grid::Grid;
pub use crate::engine::term::Term;
pub mod ansi;
