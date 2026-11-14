#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::redundant_pub_crate
)]
//! Claw sidebar shell, virtualized conversation history factory, and custom block renderers.
//!
//! Public API surface consumed by `boxxy-window` (`ClawSidebarComponent`) and
//! `boxxy-claw-widget` (`create_claw_message_list`, `get_claw_viewer_registry`,
//! `row_object::ClawRowObject`, and read-only `add_*_row` helpers).

pub(crate) mod character_style;
mod message_list;
mod process_renderer;
pub mod row_object;
pub mod sidebar;
pub mod trace_widget;

pub use character_style::{
    character_btn_color_class, claw_badge_bg_color_class, claw_pill_spinner_color_class,
    format_relative_age, is_safe_hex_color, load_character_avatar_texture, prettify_character_name,
    resolve_character_display_name,
};
pub use message_list::{
    add_approval_row, add_clipboard_get_approval_row, add_clipboard_set_approval_row,
    add_diagnosis_row, add_diagnosis_row_with_trace, add_file_delete_approval_row,
    add_kill_process_approval_row, add_suggested_row_with_trace, add_system_message_row,
    add_user_row, add_working_turn_row, apply_code_blocks_visibility_policy,
    create_claw_message_list, format_exchange_for_copy,
};
pub use process_renderer::{ProcessListRenderer, get_claw_viewer_registry};
pub use sidebar::ClawSidebarComponent;
