//! The logic of the main window, with no interface in it: what is installed, what
//! installing would change, the delegation report. The Slint and Tauri interfaces
//! each copy these views into their own widgets.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::choices::Choices;
use crate::clients::{Env, Integration, by_name};
use crate::files::Session;
use crate::handoff::State;
use crate::install::{self, Outcome};
use crate::lang::Lang;
use crate::overview::{self, Note};
use crate::parts::Kind;
use crate::{report, tr};

/// What the window works on.
pub struct Panel {
    env: Env,
    data_dir: PathBuf,
    lang: Lang,
    /// The clients found on this machine, with whether each is ticked for install.
    candidates: Vec<(&'static str, bool)>,
}

/// A client found on this machine, as the install page lists it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Choice {
    pub label: String,
    pub selected: bool,
}

/// One client's state: 1 in place, 0 missing, -1 not applicable.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ClientRow {
    pub label: String,
    pub subagents: i32,
    pub instructions: i32,
    pub hooks: i32,
    pub quota: i32,
    pub handoff: i32,
    pub notes: String,
}

/// The status page.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Status {
    pub clients: Vec<ClientRow>,
    pub needs_fix: bool,
    pub message: String,
    pub gauge_with_sessions: bool,
    pub handoff_hint: String,
}

/// One line of the delegation report; the shares are percentages, -1 when there is
/// nothing to divide.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ReportRow {
    pub agent: String,
    pub delegations: i32,
    pub escalated: i32,
    pub escalated_share: f32,
    pub without_footer: i32,
    pub without_footer_share: f32,
}

/// The delegations page.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Report {
    pub period: String,
    pub rows: Vec<ReportRow>,
    pub journal: String,
}

/// What installing the ticked clients would change.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Plan {
    pub planned: Vec<String>,
    pub can_apply: bool,
    pub message: String,
}

impl Panel {
    pub fn new(env: Env, data_dir: PathBuf, lang: Lang) -> Panel {
        let mut panel = Panel {
            env,
            data_dir,
            lang,
            candidates: Vec::new(),
        };
        panel.reset_candidates();
        panel
    }

    pub fn env(&self) -> &Env {
        &self.env
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn lang(&self) -> Lang {
        self.lang
    }

    /// Tick every client found; in manual mode the user unticks the ones to leave out.
    pub fn reset_candidates(&mut self) {
        self.candidates = install::detected(&self.env)
            .iter()
            .map(|integration| (integration.name, true))
            .collect();
    }

    /// Tick or untick the client at `index`.
    pub fn toggle(&mut self, index: usize, checked: bool) {
        if let Some((_, selected)) = self.candidates.get_mut(index) {
            *selected = checked;
        }
    }

    fn chosen(&self) -> Vec<Integration> {
        self.candidates
            .iter()
            .filter(|(_, selected)| *selected)
            .filter_map(|(name, _)| by_name(name))
            .collect()
    }

    pub fn choices(&self) -> Vec<Choice> {
        self.candidates
            .iter()
            .filter_map(|(name, selected)| {
                by_name(name).map(|integration| Choice {
                    label: integration.label.into(),
                    selected: *selected,
                })
            })
            .collect()
    }

    pub fn status(&self) -> Status {
        let lang = self.lang;
        let choices = Choices::effective(&self.data_dir, &self.env);
        let clients = overview::clients(&self.env, choices.as_ref());
        let state = |client: &overview::ClientState, kind: Kind| match client
            .status
            .iter()
            .find(|(k, _)| *k == kind)
        {
            Some((_, true)) => 1,
            Some((_, false)) => 0,
            None => -1,
        };
        let rows: Vec<ClientRow> = clients
            .iter()
            .map(|client| ClientRow {
                label: client.integration.label.into(),
                subagents: state(client, Kind::Subagents),
                instructions: state(client, Kind::Instructions),
                hooks: state(client, Kind::Hooks),
                quota: state(client, Kind::Quota),
                handoff: match client.handoff {
                    State::Connected => 1,
                    State::NotConnected => 0,
                    State::Absent => -1,
                },
                notes: client
                    .notes()
                    .into_iter()
                    .map(|note| note_text(note, lang))
                    .collect::<Vec<_>>()
                    .join("; "),
            })
            .collect();
        let needs_fix = clients.iter().any(overview::ClientState::needs_fix);
        let message = if clients.is_empty() {
            tr!(
                lang,
                "No supported AI client was found on this machine.",
                "Aucun client IA compatible n'a été trouvé sur cette machine."
            )
        } else if choices.is_none() {
            tr!(
                lang,
                "Vario is not set up yet: see the Install tab.",
                "Vario n'est pas encore configuré : voir l'onglet Installation."
            )
        } else if needs_fix {
            tr!(
                lang,
                "Something is missing: click Repair.",
                "Il manque quelque chose : cliquez sur Réparer."
            )
        } else {
            tr!(lang, "Everything is in place.", "Tout est en place.")
        };
        let handoff_hint = if clients.iter().any(|client| client.handoff == State::Absent) {
            tr!(
                lang,
                "Tip: Handoff, by the same author, keeps each project's memory when you switch from one AI agent to another (pipx install ai-handoff, Python 3.10 or later).",
                "Conseil : Handoff, du même auteur, garde la mémoire de chaque projet quand vous passez d'un agent IA à un autre (pipx install ai-handoff, Python 3.10 ou plus récent)."
            )
        } else {
            String::new()
        };
        Status {
            clients: rows,
            needs_fix: needs_fix && choices.is_some(),
            message,
            gauge_with_sessions: Choices::gauge_with_sessions(&self.data_dir),
            handoff_hint,
        }
    }

    pub fn report(&self) -> Report {
        let lang = self.lang;
        let journal = shorten(&report::journal_path(&self.data_dir), &self.env.home);
        let journal = tr!(lang, "Journal: {journal}", "Journal : {journal}");
        let summary = match report::summarize(&self.data_dir) {
            Ok(summary) => summary,
            Err(err) => {
                return Report {
                    period: err.text(lang),
                    rows: Vec::new(),
                    journal,
                };
            }
        };
        let share = |count: usize, total: usize| {
            if total == 0 {
                -1.0
            } else {
                count as f32 * 100.0 / total as f32
            }
        };
        let mut rows: Vec<ReportRow> = summary
            .agents
            .iter()
            .map(|(agent, counts)| (agent.clone(), counts))
            .chain(std::iter::once((
                tr!(lang, "Total", "Total"),
                &summary.total,
            )))
            .map(|(agent, counts)| ReportRow {
                agent: agent.to_uppercase(),
                delegations: counts.delegations as i32,
                escalated: counts.escalated as i32,
                escalated_share: share(counts.escalated, counts.delegations),
                without_footer: counts.without_footer as i32,
                without_footer_share: share(counts.without_footer, counts.delegations),
            })
            .collect();
        let period = if summary.total.delegations == 0 {
            rows.clear();
            tr!(
                lang,
                "No delegation recorded yet.",
                "Aucune délégation notée pour l'instant."
            )
        } else {
            let day = |time: &Option<String>| -> String {
                time.as_deref()
                    .map(|time| time.chars().take(10).collect())
                    .unwrap_or_default()
            };
            let (first, last) = (day(&summary.first), day(&summary.last));
            tr!(
                lang,
                "Delegations from {first} to {last}",
                "Délégations du {first} au {last}"
            )
        };
        Report {
            period,
            rows,
            journal,
        }
    }

    /// Show what installing the ticked clients would change, without writing anything.
    pub fn preview(&self) -> Plan {
        let lang = self.lang;
        let chosen = self.chosen();
        if chosen.is_empty() {
            return Plan {
                planned: Vec::new(),
                can_apply: false,
                message: tr!(lang, "No client is ticked.", "Aucun client n'est coché."),
            };
        }
        let mut session = Session::in_data_dir(true, &self.data_dir);
        let outcomes = install::apply(&mut session, &chosen, &self.env);
        let lines = self.lines(&outcomes);
        let can_apply = !lines.is_empty() && outcomes.iter().all(Outcome::ok);
        let message = if lines.is_empty() {
            tr!(
                lang,
                "Everything is already in place.",
                "Tout est déjà en place."
            )
        } else {
            tr!(
                lang,
                "Planned changes; each file is backed up first.",
                "Changements prévus ; chaque fichier est sauvegardé avant modification."
            )
        };
        Plan {
            planned: lines,
            can_apply,
            message,
        }
    }

    /// Install the ticked clients; the message tells what was done.
    pub fn apply(&mut self) -> String {
        let lang = self.lang;
        let chosen = self.chosen();
        let mut session = Session::in_data_dir(false, &self.data_dir);
        let outcomes = install::apply(&mut session, &chosen, &self.env);
        let names: Vec<String> = chosen.iter().map(|i| i.name.to_string()).collect();
        let declined: Vec<String> = self
            .candidates
            .iter()
            .filter(|(_, selected)| !*selected)
            .map(|(name, _)| name.to_string())
            .collect();
        let mut message = self.done(&outcomes, &session);
        if let Err(err) = Choices::remember(&self.data_dir, &names, &declined) {
            message = format!("{message}\n{}", err.text(lang));
        }
        message
    }

    /// Reinstall what is missing in the clients the user chose; `None` while Vario is
    /// not set up.
    pub fn repair(&mut self) -> Option<String> {
        let choices = Choices::effective(&self.data_dir, &self.env)?;
        let targets: Vec<Integration> = overview::clients(&self.env, Some(&choices))
            .into_iter()
            .filter(|client| client.chosen && client.detected)
            .map(|client| client.integration)
            .collect();
        let mut session = Session::in_data_dir(false, &self.data_dir);
        let outcomes = install::apply(&mut session, &targets, &self.env);
        Some(self.done(&outcomes, &session))
    }

    pub fn remove_all(&mut self) -> String {
        let lang = self.lang;
        let all = crate::clients::integrations();
        let mut session = Session::in_data_dir(false, &self.data_dir);
        let outcomes = install::remove(&mut session, &all, &self.env);
        let mut message = self.done(&outcomes, &session);
        if let Err(err) = Choices::delete(&self.data_dir) {
            message = format!("{message}\n{}", err.text(lang));
        }
        message
    }

    /// Remember whether the gauge also follows the sessions.
    pub fn set_gauge_with_sessions(&self, on: bool) {
        let _ = Choices::set_gauge_with_sessions(&self.data_dir, on);
    }

    /// One line per change or error, with the client's name.
    fn lines(&self, outcomes: &[Outcome]) -> Vec<String> {
        let lang = self.lang;
        let mut lines = Vec::new();
        for outcome in outcomes {
            for change in &outcome.changes {
                let path = shorten(&change.path, &self.env.home);
                let action = change.action.text(lang);
                lines.push(format!("{} — {path} : {action}", outcome.label));
            }
            for error in &outcome.errors {
                lines.push(format!(
                    "{} — ⚠ {}",
                    outcome.label,
                    error
                        .text(lang)
                        .replace(&self.env.home.display().to_string(), "~")
                ));
            }
        }
        lines
    }

    fn done(&self, outcomes: &[Outcome], session: &Session) -> String {
        let lang = self.lang;
        let count = session.changes().len();
        let mut message = if outcomes.iter().all(Outcome::ok) {
            tr!(
                lang,
                "Done: {count} change(s).",
                "Terminé : {count} changement(s)."
            )
        } else {
            let errors: Vec<String> = self
                .lines(outcomes)
                .into_iter()
                .filter(|line| line.contains('⚠'))
                .collect();
            let errors = errors.join("\n");
            tr!(
                lang,
                "Done with errors: {count} change(s).\n{errors}",
                "Terminé avec des erreurs : {count} changement(s).\n{errors}"
            )
        };
        if count > 0 {
            let folder = shorten(session.backup_dir(), &self.env.home);
            message += &tr!(lang, "\nBackups: {folder}", "\nSauvegardes : {folder}");
        }
        message
    }
}

fn note_text(note: Note, lang: Lang) -> String {
    match note {
        Note::NotFound => tr!(lang, "client not found", "client introuvable"),
        Note::Incomplete => tr!(
            lang,
            "incomplete: click Repair",
            "incomplet : cliquez sur Réparer"
        ),
        Note::Declined => tr!(
            lang,
            "set aside: tick it in Install to add it",
            "écarté : cochez-le dans Installation pour l'ajouter"
        ),
        Note::NotConfigured => tr!(
            lang,
            "not configured: see Install",
            "non configuré : voir Installation"
        ),
        Note::ConnectHandoff => tr!(
            lang,
            "connect Handoff: handoff doctor --fix",
            "connecter Handoff : handoff doctor --fix"
        ),
    }
}

/// Paths under the home directory are shown with `~`.
fn shorten(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if !home.as_os_str().is_empty() => {
            format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display())
        }
        _ => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const USER_RULES: &str = "# My rules\n";

    /// A home where Claude Code is installed, with the user's own `CLAUDE.md`.
    struct Fixture {
        _dir: tempfile::TempDir,
        env: Env,
        data_dir: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let home = dir.path().join("home");
            fs::create_dir_all(home.join(".claude")).unwrap();
            fs::write(home.join(".claude").join("CLAUDE.md"), USER_RULES).unwrap();
            Fixture {
                data_dir: dir.path().join("data"),
                env: Env {
                    home,
                    ..Env::default()
                },
                _dir: dir,
            }
        }

        fn panel(&self) -> Panel {
            Panel::new(self.env.clone(), self.data_dir.clone(), Lang::En)
        }

        fn claude(&self) -> PathBuf {
            self.env.home.join(".claude")
        }
    }

    #[test]
    fn a_fresh_machine_is_not_set_up_and_has_changes_to_apply() {
        let fixture = Fixture::new();
        let panel = fixture.panel();
        assert!(panel.status().message.contains("not set up yet"));
        assert_eq!(
            panel.choices(),
            vec![Choice {
                label: "Claude Code".into(),
                selected: true
            }]
        );
        let plan = panel.preview();
        assert_eq!(plan.planned.len(), 5, "{:?}", plan.planned);
        assert!(plan.can_apply);
        assert!(!fixture.claude().join("agents").exists());
    }

    #[test]
    fn applying_puts_everything_in_place() {
        let fixture = Fixture::new();
        let mut panel = fixture.panel();
        let message = panel.apply();
        assert!(message.starts_with("Done: 5 change(s)."), "{message}");
        let status = panel.status();
        assert_eq!(status.message, "Everything is in place.");
        assert!(!status.needs_fix);
        let row = &status.clients[0];
        assert_eq!(
            (row.subagents, row.instructions, row.hooks, row.quota),
            (1, 1, 1, 1)
        );
        assert_eq!(row.handoff, -1);
        assert!(!panel.preview().can_apply);
    }

    #[test]
    fn remove_all_restores_the_users_rules() {
        let fixture = Fixture::new();
        let mut panel = fixture.panel();
        panel.apply();
        panel.remove_all();
        let rules = fs::read_to_string(fixture.claude().join("CLAUDE.md")).unwrap();
        assert_eq!(rules, USER_RULES);
        assert!(
            !fixture
                .claude()
                .join("agents")
                .join("vario-reader.md")
                .exists()
        );
        assert!(!fixture.data_dir.join("setup.json").exists());
    }

    #[test]
    fn an_unticked_client_has_nothing_to_preview() {
        let fixture = Fixture::new();
        let mut panel = fixture.panel();
        panel.toggle(0, false);
        let plan = panel.preview();
        assert!(plan.planned.is_empty());
        assert!(!plan.can_apply);
        assert_eq!(plan.message, "No client is ticked.");
        panel.reset_candidates();
        assert!(panel.choices()[0].selected);
    }

    #[test]
    fn repair_waits_for_a_setup_then_reports() {
        let fixture = Fixture::new();
        let mut panel = fixture.panel();
        assert_eq!(panel.repair(), None);
        panel.apply();
        fs::remove_file(fixture.claude().join("agents").join("vario-reader.md")).unwrap();
        assert!(panel.status().needs_fix);
        let message = panel.repair().expect("a message");
        assert!(message.starts_with("Done: 1 change(s)."), "{message}");
        assert!(!panel.status().needs_fix);
    }

    #[test]
    fn the_report_reads_the_journal() {
        let fixture = Fixture::new();
        let empty = fixture.panel().report();
        assert_eq!(empty.period, "No delegation recorded yet.");
        assert!(empty.rows.is_empty());
        fs::create_dir_all(&fixture.data_dir).unwrap();
        let lines = [
            r#"{"time":"2026-10-16T10:00:00+01:00","client":"claude","agent":"vario-worker","result":"ok","escalate":false}"#,
            r#"{"time":"2026-10-16T11:00:00+01:00","client":"claude","agent":"vario-worker","result":"stuck","escalate":true}"#,
        ];
        fs::write(
            report::journal_path(&fixture.data_dir),
            lines.join("\n") + "\n",
        )
        .unwrap();
        let report = fixture.panel().report();
        let worker = report
            .rows
            .iter()
            .find(|row| row.agent == "VARIO-WORKER")
            .expect("a vario-worker channel");
        assert_eq!((worker.delegations, worker.escalated), (2, 1));
        assert_eq!(worker.escalated_share, 50.0);
        assert!(report.rows.iter().any(|row| row.agent == "TOTAL"));
        assert!(report.period.contains("Delegations from"));
        assert!(report.journal.starts_with("Journal: "));
    }
}
