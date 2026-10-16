# Vario

[![CI](https://github.com/kdev1966/vario/actions/workflows/ci.yml/badge.svg)](https://github.com/kdev1966/vario/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://github.com/kdev1966/vario/blob/main/LICENSE)

[English](https://github.com/kdev1966/vario/blob/main/README.md) · **Français**

**Vario** fait déléguer à vos agents IA de programmation le travail simple à des modèles moins chers. Il donne à chaque agent deux sous-agents économiques et de courtes consignes de délégation, par les mécanismes propres à l'agent : pas de proxy, pas de clé d'API, rien n'est envoyé nulle part. Votre session principale garde son modèle puissant, et son cache de prompt, pour la conception, le débogage et la relecture ; la lecture, le résumé et les tâches bien définies vont à un sous-agent sur un modèle moins cher.

Il fonctionne sous **macOS, Linux et Windows**, sous forme de commande (`vario`) et d'application (`vario-gui`).

> Vario est jeune (phase 2 de sa [feuille de route](vario-document-de-conception.md), en français). La délégation a été essayée pour de vrai avec Claude Code et OpenCode. Cursor, Codex, Gemini CLI et VS Code suivent leur documentation et sont couverts par des tests ; Gemini CLI et VS Code ont été installés sur une vraie machine, sans délégation essayée pour l'instant.

## Fonctionnement

Vario installe deux sous-agents dans chaque client qu'il trouve :

| Sous-agent | Rôle | Outils |
| --- | --- | --- |
| `vario-reader` | lire et résumer : repérer du code, extraire des valeurs, résumer un fichier ou la longue sortie d'un outil | lecture seule |
| `vario-worker` | mener à bien une tâche bornée qui s'accompagne d'une vérification : renommer, appliquer un motif connu, lancer des commandes connues | lecture, édition, shell |

Chaque sous-agent termine sa réponse par un court compte rendu (`RESULT`, `CHECKS`, `UNCERTAINTIES`, `ESCALATE`). Une vérification échouée renvoie la tâche d'un niveau au-dessus, une seule fois ; la session principale décide du reste.

| Client | Sous-agents et modèle | Consignes de délégation | Hooks |
| --- | --- | --- | --- |
| **Claude Code** | `~/.claude/agents/`, `haiku` et `sonnet` | un bloc dans `~/.claude/CLAUDE.md` | fin de chaque sous-agent → journal ; avant chaque prompt → thermostat de quota ; une ligne de statut qui relève le quota |
| **Cursor** | `~/.cursor/agents/`, Composer 2.5 (le modèle propre à Cursor, sans facturation tierce) | injectées au début de la session (Cursor n'a pas de fichier de règles global) | fin de chaque sous-agent → journal |
| **OpenCode** | `~/.config/opencode/agents/`, votre `small_model` pour le lecteur, sinon le modèle de l'appelant | le fichier de règles que lit OpenCode (`AGENTS.md`, ou `~/.claude/CLAUDE.md`) | un plugin : fin de chaque sous-agent → journal |
| **Codex** | `~/.codex/agents/*.toml`, `gpt-6-luna` pour le lecteur (sans shell), le modèle de la session pour le worker | un bloc dans `~/.codex/AGENTS.md` (Codex ne délègue que si ses consignes le demandent) | fin de chaque sous-agent → journal (à approuver une fois avec `/hooks`) |
| **Gemini CLI** | `~/.gemini/agents/`, `flash-lite` pour le lecteur (outils de lecture seuls), le modèle de la session pour le worker | un bloc dans `~/.gemini/GEMINI.md` | aucun : Gemini CLI n'a pas d'événement de fin de sous-agent |
| **VS Code (Copilot)** | `~/.copilot/agents/*.agent.md`, Claude Haiku 4.5 pour le lecteur (`read`, `search`), le modèle de la conversation pour le worker | un bloc dans `~/.copilot/copilot-instructions.md` | `~/.copilot/hooks/vario.json` → journal (hooks en préversion : `chat.useHooks`) |

Codex, Gemini CLI et VS Code suivent leur documentation et sont couverts par des tests. Les sous-agents demandent un Gemini CLI récent (ils n'existent pas en 0.1.x), et Google a fait passer une partie de ses utilisateurs à Antigravity CLI.

Les hooks consignent chaque délégation aux sous-agents de Vario dans un journal local ; `vario report` et l'onglet **Délégations** de l'application le résument : délégations, escalades et réponses sans compte rendu.

**Quota et thermostat (Claude Code).** Claude Code n'indique qu'à la ligne de statut la part de votre abonnement déjà consommée. La ligne de statut de Vario la relève et affiche `Vario · 5 h 23 % · 7 j 41 %` ; si vous avez déjà une ligne de statut, Vario l'enveloppe : elle reste exactement la même. Avant chaque prompt, le thermostat regarde la fenêtre la plus consommée : dès 50 %, il demande à la session principale de déléguer davantage ; dès 80 %, il vous avertit aussi ; à 100 %, il propose de changer d'agent après avoir enregistré une passation [Handoff](https://github.com/kdev1966/Handoff-CLI). `vario usage` affiche le quota, les tokens par modèle (session principale et sous-agents) sur 5 heures et 7 jours, la part du travail déléguée, et les serveurs MCP connectés mais jamais utilisés.

Claude Code n'exécute les lignes de statut que dans le terminal : le quota est relevé quand vous utilisez Claude Code dans un terminal, pas depuis l'extension VS Code. Le reste (sous-agents, consignes, journal, `vario usage`) fonctionne partout.

**Jauge flottante.** `vario-gui --gauge`, ou **Jauge flottante** dans l'application, ouvre une petite fenêtre toujours au premier plan, dessinée comme un plugin audio : un VU-mètre à aiguille pour le quota de 5 heures, une barre pour celui de 7 jours, l'état du thermostat en boutons de mode, et deux boutons rotatifs pour la part déléguée et le taux d'escalade. Déplacez-la par son en-tête.

## Installation

**Binaires prêts à l'emploi**, sur la page [Releases](https://github.com/kdev1966/vario/releases) :

- **Application macOS** : `Vario-macos-aarch64.zip` (processeur Apple) ou `Vario-macos-x86_64.zip` (Intel). Décompressez-la et placez `Vario.app` dans Applications. Elle n'est pas notarisée : ouvrez-la la première fois par un clic droit, **Ouvrir**. La commande est incluse dans l'application.
- **Commande, macOS et Linux** :
  ```sh
  curl --proto '=https' --tlsv1.2 -LsSf https://github.com/kdev1966/vario/releases/latest/download/vario-installer.sh | sh
  ```
- **Commande, Windows** (PowerShell) :
  ```powershell
  powershell -ExecutionPolicy Bypass -c "irm https://github.com/kdev1966/vario/releases/latest/download/vario-installer.ps1 | iex"
  ```
- **Application sous Linux et Windows** : de la même façon, avec `vario-gui-installer.sh` ou `vario-gui-installer.ps1`.

**À partir des sources** (Rust 1.88 ou plus pour la commande, 1.92 pour l'application) :

```sh
cargo install --git https://github.com/kdev1966/vario vario
cargo install --git https://github.com/kdev1966/vario vario-gui   # l'application, facultative
```

Laissez ensuite Vario configurer les clients IA qu'il trouve :

```sh
vario install            # affiche les changements, puis demande avant de les appliquer
```

Pour construire `Vario.app` vous-même : `scripts/macos-app.sh`.

## Utilisation

```sh
vario install --dry-run          # ce qui changerait, sans rien changer
vario install --manual           # choisir les clients un par un
vario install --client opencode  # configurer un client, même s'il n'est pas détecté
vario doctor                     # ce qui est installé où ; --fix le répare
vario report                     # le journal des délégations, par sous-agent
vario usage                      # quota, thermostat et tokens consommés, d'après les journaux locaux
vario restore --list             # les sauvegardes ; `vario restore` remet la plus récente
vario remove                     # retirer Vario de tous les clients
```

Sans terminal pour confirmer, Vario ne change rien, sauf si vous passez `--yes`.

L'application (`vario-gui`, ou `Vario.app`) offre les mêmes fonctions en quatre onglets : **État** (avec Réparer et Retirer), **Installation** (automatique ou manuelle, les changements étant affichés avant d'être appliqués), **Délégations** et **À propos**.

Les messages suivent la langue de votre système, français ou anglais ; `VARIO_LANG=en` ou `VARIO_LANG=fr` la remplace.

## Sécurité

- **Rien ne change sans aperçu** et sans votre confirmation (ou `--yes`).
- **Chaque fichier est sauvegardé** avant sa première modification, dans le dossier de données de Vario (`~/Library/Application Support/vario` sous macOS, `~/.local/share/vario` sous Linux, `%LOCALAPPDATA%\vario` sous Windows ; `VARIO_HOME` le remplace). `vario restore` remet une sauvegarde en place.
- **Vos réglages et ceux des autres outils sont conservés** : Vario ne modifie que son propre bloc, ses propres entrées de hooks et ses propres fichiers, reconnus par un marqueur, et refuse de réécrire une configuration qu'il ne peut pas analyser exactement. Un test installe puis retire Vario sur une configuration [Handoff](https://github.com/kdev1966/Handoff-CLI) d'allure réelle dans les trois clients, et vérifie que chaque fichier revient octet pour octet.
- **Local** : Vario lit et écrit des fichiers sur votre machine, et n'envoie rien à personne.

## Outil compagnon

[**Handoff**](https://github.com/kdev1966/Handoff-CLI), du même auteur, conserve la mémoire de chaque projet quand vous passez d'un agent IA à un autre. Vario le détecte et indique s'il est connecté à chaque client ; ils s'installent et fonctionnent indépendamment.

## Développement

```sh
cargo test                  # tout l'espace de travail
cargo clippy --all-targets -- -D warnings
cargo run -p vario -- doctor
cargo run -p vario-gui
```

L'espace de travail compte trois crates : `vario-core` (toute la logique), `vario` (la commande) et `vario-gui` (l'application, construite avec [Slint](https://slint.dev)). La conception et la feuille de route se trouvent dans [vario-document-de-conception.md](vario-document-de-conception.md), en français.

## Licence

MIT. L'application est construite avec [Slint](https://slint.dev) sous sa licence gratuite, que son onglet **À propos** mentionne.
