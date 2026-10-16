//! The state of each client, as `vario doctor` and the interface show it. Each front
//! end words the [`Note`]s its own way (a command for the CLI, a button for the
//! interface).

use crate::choices::Choices;
use crate::clients::{Env, Integration, integrations};
use crate::handoff::{self, State};
use crate::install;
use crate::parts::Kind;

pub struct ClientState {
    pub integration: Integration,
    pub detected: bool,
    /// Vario's own files for this client are in place ([`install::installed`]).
    pub installed: bool,
    pub status: Vec<(Kind, bool)>,
    pub chosen: bool,
    pub declined: bool,
    pub handoff: State,
}

/// Something the user may want to act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Note {
    /// Chosen, but the client is no longer found on this machine.
    NotFound,
    /// Chosen, but something Vario installs is missing or outdated.
    Incomplete,
    /// The user chose not to configure this client.
    Declined,
    /// Found, but Vario was never set up for it.
    NotConfigured,
    /// Handoff is installed but not connected to this client.
    ConnectHandoff,
}

impl ClientState {
    pub fn complete(&self) -> bool {
        self.status.iter().all(|(_, present)| *present)
    }

    pub fn settled(&self) -> bool {
        self.chosen || self.declined
    }

    /// Whether a repair would do something: a chosen client is incomplete, or a
    /// client was found that the user has not decided about.
    pub fn needs_fix(&self) -> bool {
        (self.chosen && self.detected && !self.complete()) || (self.detected && !self.settled())
    }

    pub fn notes(&self) -> Vec<Note> {
        let mut notes = Vec::new();
        if self.chosen && !self.detected {
            notes.push(Note::NotFound);
        } else if self.chosen && !self.complete() {
            notes.push(Note::Incomplete);
        } else if self.declined {
            notes.push(Note::Declined);
        } else if !self.chosen {
            notes.push(Note::NotConfigured);
        }
        if self.handoff == State::NotConnected {
            notes.push(Note::ConnectHandoff);
        }
        notes
    }
}

/// The clients worth showing: found on this machine, chosen, or with Vario installed.
pub fn clients(env: &Env, choices: Option<&Choices>) -> Vec<ClientState> {
    let has = |list: fn(&Choices) -> &Vec<String>, name: &str| {
        choices.is_some_and(|choices| list(choices).iter().any(|n| n == name))
    };
    integrations()
        .into_iter()
        .map(|integration| ClientState {
            detected: integration.detect(env),
            installed: install::installed(&integration, env),
            status: install::status(&integration, env),
            chosen: has(|c| &c.clients, integration.name),
            declined: has(|c| &c.declined, integration.name),
            handoff: handoff::state(&integration, env),
            integration,
        })
        .filter(|client| client.detected || client.chosen || client.installed)
        .collect()
}
