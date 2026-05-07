//! `boxxy_claw_widget` — the reusable Claw drawer UI.
//!
//! Consumers interact with the widget through the `ClawHost` trait —
//! the drawer has no dependency on terminal-specific types and can
//! host any surface that implements `ClawHost` (terminal panes today;
//! standalone chat windows or mobile shells in the future).

pub mod claw_host;
pub mod claw_indicator;
pub mod dispatch;
pub mod msgbar;
pub mod overlay;
pub mod proposal;
pub mod tips;

pub use tips::TipsCycle;

pub use claw_host::ClawHost;
pub use claw_indicator::ClawIndicator;
pub use dispatch::spawn_dispatch;
pub use msgbar::{Attachment, AttachmentManager, MsgBarComponent};
pub use overlay::{OverlayMode, TerminalOverlay};
pub use proposal::Proposal;
