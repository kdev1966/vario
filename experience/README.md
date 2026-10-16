# Test d'économie — en continu pendant le développement

Ce test vérifie que la délégation à des sous-agents économiques réduit la consommation de quota de Claude Code sans perte de qualité. On reproduit à la main ce que Vario ferait en phase 1, pendant le développement de Vario lui-même, sans le ralentir.

## Critère de réussite (fixé le 16 octobre 2026, avant le test)

- **Économie** : au moins 20 % de quota en moins par heure de travail, par rapport à l'historique des sessions sans délégation.
- **Fiabilité** : au plus une délégation ratée ou refaite sur cinq.
- **Qualité** : aucune régression constatée dans le travail rendu.

Les trois conditions doivent être remplies. Sinon, on analyse pourquoi avant de généraliser la politique de délégation.

## Dispositif

| Fichier | Rôle |
| --- | --- |
| `agents/vario-lecteur.md` | sous-agent économique (`haiku`), lecture seule : lire et résumer |
| `agents/vario-executant.md` | sous-agent intermédiaire (`sonnet`) : exécuter une tâche cadrée, vérification obligatoire |
| `delegation.md` | consignes pour la session principale : quand déléguer, formats, escalade |
| `journal.md` | une ligne par délégation |

Activation, faite le 16 octobre 2026 : les deux sous-agents sont copiés dans `.claude/agents/`, et le `CLAUDE.md` du projet importe `@experience/delegation.md`. Les sous-agents sont actifs en permanence pendant le test.

## Ce qui ne change pas pendant le test

La configuration globale (`~/.claude/CLAUDE.md`, serveurs MCP) et le modèle de la session principale (Opus 5.5). Sinon, on ne saura pas ce qui a fait varier la consommation.

## Mesures

- **Pendant le test** : la session principale ajoute une ligne à `journal.md` après chaque délégation. Ce coût en tokens est faible, mais il compte dans la mesure.
- **Après deux semaines** : tokens et quota par heure tirés des journaux locaux de Claude Code (`~/.claude/projects/`), avec Paceline ou un petit script de lecture, comparés à l'historique antérieur.
- **Limite assumée** : l'historique porte sur d'autres projets et d'autres charges de travail ; la comparaison est indicative, pas une preuve rigoureuse.

## Désactiver

```sh
rm -f .claude/agents/vario-*.md
```

Puis retirer la ligne `@experience/delegation.md` du `CLAUDE.md`, sans toucher au reste du fichier.
