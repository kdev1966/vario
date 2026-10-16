//! Detection of Handoff, the companion tool that keeps a project's memory across
//! agents. Vario does not depend on it: it only reports its state, so the CLI can
//! suggest installing or connecting it.

use crate::clients::{Env, Integration};

/// The command Handoff installs (`pipx install ai-handoff`).
pub const COMMAND: &str = "handoff";
/// The name Handoff registers its MCP server under in each client.
pub const SERVER_NAME: &str = "handoff";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Absent,
    /// Installed, but its MCP server is not registered with this client.
    NotConnected,
    Connected,
}

pub fn installed(env: &Env) -> bool {
    env.which(COMMAND).is_some()
}

/// Handoff's state as seen from one client.
pub fn state(integration: &Integration, env: &Env) -> State {
    if integration.handoff_connected(env) {
        State::Connected
    } else if installed(env) {
        State::NotConnected
    } else {
        State::Absent
    }
}
