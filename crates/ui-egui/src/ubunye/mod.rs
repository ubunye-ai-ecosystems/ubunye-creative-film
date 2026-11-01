//! Ubunye additions to the UI (new files only; upstream files call in with one line each).
//!
//! - [`kopano`]: the client for `kopanod`, the collaboration service on this computer.
//! - [`kopano_panel`]: the Share & Collaborate window (Window ▸ Share & Collaborate, and the
//!   people icon in the header) and its commands.

pub mod kopano;
pub mod kopano_panel;

#[cfg(test)]
mod tests_kopano;
