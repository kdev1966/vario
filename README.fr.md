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
| **Claude Code** | `~/.claude/agents/`, `haiku` et `sonnet` | un bloc dans `~/.claude/CLAUDE.md` | début de session → jauge ; envoi d'un prompt → agent actif ; fin de chaque sous-agent → journal ; avant chaque prompt → thermostat de quota ; une ligne de statut qui relève le quota |
| **Cursor** | `~/.cursor/agents/`, Composer 2.5 (le modèle propre à Cursor, sans facturation tierce) | injectées au début de la session (Cursor n'a pas de fichier de règles global) | début de session → consignes et jauge ; fin de chaque sous-agent → journal |
| **OpenCode** | `~/.config/opencode/agents/`, votre `small_model` pour le lecteur, sinon le modèle de l'appelant | le fichier de règles que lit OpenCode (`AGENTS.md`, ou `~/.claude/CLAUDE.md`) | un plugin : envoi d'un prompt → agent actif ; réponse terminée → actualisation de l'usage ; démarrage → jauge ; fin de chaque sous-agent → journal |
| **Codex** | `~/.codex/agents/*.toml`, `gpt-6-luna` pour le lecteur (sans shell), le modèle de la session pour le worker | un bloc dans `~/.codex/AGENTS.md` (Codex ne délègue que si ses consignes le demandent) | envoi d'un prompt → agent actif ; début de session → jauge ; fin de chaque sous-agent → journal (à approuver une fois avec `/hooks`) |
| **Gemini CLI** | `~/.gemini/agents/`, `flash-lite` pour le lecteur (outils de lecture seuls), le modèle de la session pour le worker | un bloc dans `~/.gemini/GEMINI.md` | aucun : Gemini CLI n'a pas d'événement de fin de sous-agent |
| **VS Code (Copilot)** | `~/.copilot/agents/*.agent.md`, Claude Haiku 4.5 pour le lecteur (`read`, `search`), le modèle de la conversation pour le worker | un bloc dans `~/.copilot/copilot-instructions.md` | `~/.copilot/hooks/vario.json` : envoi d'un prompt → agent actif ; réponse terminée → actualisation de l'usage ; début de session → jauge ; fin de chaque sous-agent → journal (hooks en préversion : `chat.useHooks`) |
| **Kiro CLI** | `~/.kiro/cli-agents/*.json` (ou `~/.aws/amazonq/cli-agents/` pour Amazon Q CLI), `claude-haiku-4.5` pour le lecteur (`fs_read`, `fs_write`), le modèle de la session pour le worker | le champ `prompt` de l'agent | `agentSpawn` → jauge ; `userPromptSubmit` → agent actif |

Codex, Gemini CLI, VS Code et Kiro CLI suivent leur documentation et sont couverts par des tests. Les sous-agents demandent un Gemini CLI récent (ils n'existent pas en 0.1.x), et Google a fait passer une partie de ses utilisateurs à Antigravity CLI. Kiro CLI est le successeur, à code fermé, d'Amazon Q Developer CLI ; Vario s'installe dans `~/.kiro/cli-agents/`, ou dans `~/.aws/amazonq/cli-agents/` quand seul l'ancien chemin existe.

**Changement d'agent dans VS Code.** Vario bascule la jauge lorsqu'un agent pris en charge signale l'envoi d'un prompt : Claude Code, Codex, OpenCode ou GitHub Copilot. Pour CodeGeeX, Vario surveille en lecture seule l'historique local des conversations dans le stockage VS Code (sans conserver le contenu des prompts). Activez le paramètre VS Code `chat.useHooks` pour les hooks Copilot ; approuvez une fois les hooks Codex avec `/hooks`. L'API publique de VS Code ne fournit pas d'événement universel signalant la sélection d'une conversation d'une autre extension ; sélectionner un chat sans envoyer de prompt ne peut donc pas encore basculer la jauge. Amazon Q n'est pas encore intégré.

Les hooks consignent chaque délégation aux sous-agents de Vario dans un journal local ; `vario report` et l'onglet **Délégations** de l'application le résument : délégations, escalades et réponses sans compte rendu.

**Quota et thermostat (Claude Code).** Claude Code n'indique qu'à la ligne de statut la part de votre abonnement déjà consommée. La ligne de statut de Vario la relève et affiche `Vario · 5 h 23 % · 7 j 41 %` ; si vous avez déjà une ligne de statut, Vario l'enveloppe : elle reste exactement la même. Avant chaque prompt, le thermostat regarde la fenêtre la plus consommée : dès 50 %, il demande à la session principale de déléguer davantage ; dès 80 %, il vous avertit aussi ; à 100 %, il propose de changer d'agent après avoir enregistré une passation [Handoff](https://github.com/kdev1966/Handoff-CLI). `vario usage` affiche le quota, les tokens par modèle (session principale et sous-agents) sur 5 heures et 7 jours, la part du travail déléguée, et les serveurs MCP connectés mais jamais utilisés.

Claude Code n'exécute les lignes de statut que dans le terminal : le quota est relevé quand vous utilisez Claude Code dans un terminal, pas depuis l'extension VS Code. Le reste (sous-agents, consignes, journal, `vario usage`) fonctionne partout.

**Jauge flottante.** `vario-gui --gauge`, ou **Jauge flottante** dans l'application, ouvre une fenêtre système étroite (156 px), sombre, flottante et sans bordure, dessinée comme un plugin audio et assez fine pour tenir à côté de votre éditeur : au-dessus des autres applications, sur tous les bureaux (et par-dessus les applications en plein écran sur macOS), sans icône dans le Dock ni dans la barre des tâches en mode `--gauge`. Elle montre un VU-mètre à aiguille pour le quota de 5 heures, une rangée de diodes pour celui de 7 jours, l'état du thermostat en voyants, et deux boutons rotatifs pour la part déléguée et le taux d'escalade. Déplacez-la en la saisissant n'importe où.

Elle s'ouvre d'elle-même au début de la session d'un agent, dans chaque client dont Vario a installé les hooks (tous sauf Gemini CLI), et reste affichée quand l'agent se ferme ; décochez **Ouvrir la jauge avec chaque agent** dans la page **Installation** pour ne l'ouvrir qu'à la demande. Quand un client expose un hook d'envoi de prompt, la jauge bascule vers lui dès l'envoi ; sélectionner simplement un autre onglet de chat ne déclenche pas nécessairement le changement. Elle revient là où vous l'avez laissée, sinon dans le coin en haut à droite de l'écran. Une seule tourne à la fois : la rouvrir, ou ouvrir l'application, fait apparaître celle qui tourne. Son lien **ouvrir Vario** ouvre la fenêtre principale à côté d'elle. Lâchées près l'une de l'autre, les deux fenêtres s'aimantent, alignées par le haut ; déplacer la jauge emmène alors la fenêtre principale, et éloigner la fenêtre principale la détache. Fermer la fenêtre principale laisse la jauge affichée.

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

L'application (`vario-gui`, ou `Vario.app`) offre les mêmes fonctions dans un panneau sombre de la hauteur de la jauge, en quatre pages : **État** (une carte de voyants par client, avec Réparer et Retirer), **Installation** (automatique ou manuelle, les changements étant affichés avant d'être appliqués, et le réglage de la jauge), **Délégations** (une tranche de boutons rotatifs par sous-agent) et **À propos**.

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
