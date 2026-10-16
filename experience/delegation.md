# Délégation (test d'économie Vario)

Délègue ce qui peut l'être ; garde ici la conception, le débogage, les arbitrages et la relecture.

- Lire et résumer → `vario-reader`.
- Exécuter une tâche cadrée avec une vérification → `vario-worker`.
- Restent ici : sécurité, opérations destructrices, demandes ambiguës, tout ce qui demande le jugement de l'utilisateur.

Consigne de délégation, des pointeurs et jamais de contenu recopié :

```
OBJECTIF : <résultat attendu, une phrase>
ENTRÉES : <fichiers et lignes, identifiants, paramètres>
CONTRAINTES : <ce qu'il ne faut pas faire, format attendu>
VÉRIFICATION : <commande ou critère de réussite>
```

Escalade : « ESCALADER : oui » n'est qu'un indice. Vérification échouée → relancer une fois au niveau au-dessus (`vario-reader` → `vario-worker` en lecture seule ; `vario-worker` → tu reprends la tâche). Sans vérification possible, relis : insuffisant → tu reprends la tâche.

Après chaque délégation, ajoute une ligne au tableau « Délégations » de `experience/journal.md`.
