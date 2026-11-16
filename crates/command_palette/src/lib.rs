#![allow(clippy::redundant_pub_crate)]

mod catalog;
mod component;
mod search;

pub use catalog::{
    CommandItem, CommandKind, OPEN_PREFERENCES_PAGE_ACTION, PREFERENCES_CATEGORIES,
    PreferencesCategorySpec,
};
pub use component::CommandPaletteComponent;
