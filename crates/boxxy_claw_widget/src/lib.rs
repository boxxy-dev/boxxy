#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::redundant_pub_crate
)]
//! `boxxy_claw_widget` — the reusable Claw drawer UI.
//!
//! Consumers interact with the widget through the `ClawHost` trait —
//! the drawer has no dependency on terminal-specific types and can
//! host any surface that implements `ClawHost` (terminal panes today;
//! standalone chat windows or mobile shells in the future).

pub mod btw_dialog;
pub(crate) mod character_picker;
pub mod claw_host;
pub mod claw_indicator;
pub mod dispatch;
pub mod msgbar;
pub mod overlay;
pub mod proposal;
pub mod tips;
pub mod zoom;

pub use tips::TipsCycle;

pub use btw_dialog::BtwController;
pub use claw_host::ClawHost;
pub use claw_indicator::ClawIndicator;
pub use dispatch::spawn_dispatch;
pub use msgbar::{Attachment, AttachmentManager, AttachmentPayload, MsgBarComponent};
pub use overlay::{OverlayMode, TerminalOverlay};
pub use proposal::Proposal;
pub mod state;
pub use state::OverlayState;
pub use zoom::{reset_overlay_zoom, set_overlay_zoom_pct, zoom_in_overlay, zoom_out_overlay};
