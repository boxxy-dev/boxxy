//! `From<TerminalProposal> for boxxy_claw_widget::Proposal` — the single
//! boundary conversion from the terminal's producer-shape proposal enum
//! to the widget's UI-shape consumer-shape.
//!
//! The enum itself lives in `boxxy_claw_widget` (host-agnostic). The
//! impl stays here because the orphan rule wants either the trait or
//! the *target type* to be local — and neither is local to
//! `boxxy_claw_widget` (which doesn't know about `TerminalProposal`).
//! In the terminal crate, `TerminalProposal` is local, so the impl
//! compiles here.

use crate::TerminalProposal;
use boxxy_claw_widget::Proposal;

impl From<TerminalProposal> for Proposal {
    fn from(p: TerminalProposal) -> Self {
        match p {
            TerminalProposal::None => Proposal::None,
            TerminalProposal::Command(cmd) => Proposal::Command(cmd),
            TerminalProposal::Bookmark(filename, script, placeholders) => Proposal::Bookmark {
                filename,
                script,
                placeholders,
            },
            TerminalProposal::FileWrite(path, content) => Proposal::FileWrite {
                path,
                content,
                diff: None,
            },
            TerminalProposal::FileDelete(path) => Proposal::FileDelete { path },
            TerminalProposal::KillProcess(pid, name) => Proposal::KillProcess { pid, name },
            TerminalProposal::GetClipboard => Proposal::GetClipboard,
            TerminalProposal::SetClipboard(text) => Proposal::SetClipboard(text),
        }
    }
}
