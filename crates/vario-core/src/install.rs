//! Applying, removing and checking integrations. Nothing here runs on its own: the
//! CLI calls it with what the user chose, after a preview.

use crate::clients::{Env, Integration, integrations};
use crate::error::Error;
use crate::files::{Change, Session};
use crate::parts::Kind;

/// What happened to one client.
#[derive(Debug)]
pub struct Outcome {
    pub name: &'static str,
    pub label: &'static str,
    pub changes: Vec<Change>,
    /// Applying stops at the first error; removing attempts every part.
    pub errors: Vec<Error>,
}

impl Outcome {
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }
}

pub fn detected(env: &Env) -> Vec<Integration> {
    integrations()
        .into_iter()
        .filter(|integration| integration.detect(env))
        .collect()
}

/// Configure each client; the first failing part stops that client, not the others.
pub fn apply(session: &mut Session, chosen: &[Integration], env: &Env) -> Vec<Outcome> {
    chosen
        .iter()
        .map(|integration| {
            let before = session.changes().len();
            let errors = integration
                .parts(env)
                .iter()
                .try_for_each(|part| part.apply(session))
                .err()
                .into_iter()
                .collect();
            outcome(integration, session, before, errors)
        })
        .collect()
}

/// Remove Vario from each client; every part is attempted even when one fails.
pub fn remove(session: &mut Session, chosen: &[Integration], env: &Env) -> Vec<Outcome> {
    chosen
        .iter()
        .map(|integration| {
            let before = session.changes().len();
            let errors = integration
                .parts(env)
                .iter()
                .filter_map(|part| part.remove(session).err())
                .collect();
            outcome(integration, session, before, errors)
        })
        .collect()
}

fn outcome(
    integration: &Integration,
    session: &Session,
    before: usize,
    errors: Vec<Error>,
) -> Outcome {
    Outcome {
        name: integration.name,
        label: integration.label,
        changes: session.changes()[before..].to_vec(),
        errors,
    }
}

/// Whether Vario is installed in this client: its sub-agent files, which belong to
/// this client alone, are in place. A shared file such as `CLAUDE.md`, which OpenCode
/// also reads, does not count: another client may have filled it.
pub fn installed(integration: &Integration, env: &Env) -> bool {
    status(integration, env).contains(&(Kind::Subagents, true))
}

/// For each kind this client supports: whether everything of that kind is in place.
pub fn status(integration: &Integration, env: &Env) -> Vec<(Kind, bool)> {
    let mut result: Vec<(Kind, bool)> = Vec::new();
    for part in integration.parts(env) {
        let present = part.present();
        match result.iter_mut().find(|(kind, _)| *kind == part.kind()) {
            Some((_, all)) => *all = *all && present,
            None => result.push((part.kind(), present)),
        }
    }
    result
}
