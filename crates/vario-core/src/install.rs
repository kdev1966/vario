//! Applying, removing and checking integrations. Nothing here runs on its own: the
//! CLI calls it with what the user chose, after a preview.

use crate::clients::{Env, Integration, integrations};
use crate::files::Session;
use crate::parts::Kind;

/// What happened to one client.
#[derive(Debug)]
pub struct Outcome {
    pub name: &'static str,
    pub label: &'static str,
    pub changes: Vec<String>,
    pub error: Option<String>,
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
            let error = integration
                .parts(env)
                .iter()
                .try_for_each(|part| part.apply(session))
                .err()
                .map(|err| err.to_string());
            outcome(integration, session, before, error)
        })
        .collect()
}

/// Remove Vario from each client; every part is attempted even when one fails.
pub fn remove(session: &mut Session, chosen: &[Integration], env: &Env) -> Vec<Outcome> {
    chosen
        .iter()
        .map(|integration| {
            let before = session.changes().len();
            let errors: Vec<String> = integration
                .parts(env)
                .iter()
                .filter_map(|part| part.remove(session).err())
                .map(|err| err.to_string())
                .collect();
            let error = (!errors.is_empty()).then(|| errors.join("; "));
            outcome(integration, session, before, error)
        })
        .collect()
}

fn outcome(
    integration: &Integration,
    session: &Session,
    before: usize,
    error: Option<String>,
) -> Outcome {
    Outcome {
        name: integration.name,
        label: integration.label,
        changes: session.changes()[before..].to_vec(),
        error,
    }
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
