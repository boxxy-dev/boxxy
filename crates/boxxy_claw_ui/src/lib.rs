//! Claw sidebar shell, virtualized conversation history factory, and custom block renderers.
//!
//! Public API surface consumed by `boxxy-window` (`ClawSidebarComponent`) and
//! `boxxy-claw-widget` (`create_claw_message_list`, `get_claw_viewer_registry`,
//! `row_object::ClawRowObject`, and read-only `add_*_row` helpers).

mod character_style;
mod message_list;
mod process_renderer;
pub mod row_object;
pub mod sidebar;

pub use character_style::{character_btn_color_class, load_character_avatar_texture};
pub use message_list::{
    add_approval_row, add_clipboard_get_approval_row, add_clipboard_set_approval_row,
    add_diagnosis_row, add_diagnosis_row_with_typewriter, add_file_delete_approval_row,
    add_file_diff_row, add_file_diff_row_with_mode, add_file_write_approval_row,
    add_kill_process_approval_row, add_process_list_row, add_suggested_row, add_system_message_row,
    add_tool_call_row, add_user_row, create_claw_message_list, format_exchange_for_copy,
};
pub use process_renderer::{ProcessListRenderer, get_claw_viewer_registry};
pub use sidebar::ClawSidebarComponent;
