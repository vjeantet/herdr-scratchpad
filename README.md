# herdr-scratchpad

**Le presse-papier qui se souvient.**

Un panneau [herdr](https://herdr.dev) docké en bas où l'on colle, où l'on
récupère, et qu'on vide. Pas un carnet : un outil de **transit**. Le texte est
sauvegardé tout seul, en texte brut, et revient après un redémarrage.

```
herdr plugin link .
```

## Les quatre touches

| Touche | Bouton | Action |
| --- | --- | --- |
| `Ctrl+C` | `^C copier` | copie tout vers le presse-papier de ta machine |
| `Ctrl+L` | `^L vider` | vide, sans confirmation |
| `Ctrl+S` | `^S fichier` | écrit `/tmp/herdr-scratchpad.txt` |
| `Ctrl+Z` | `^Z annuler` | ramène le dernier contenu vidé |

Ce sont les combinaisons que tes doigts connaissent déjà, chacune à sa
signification habituelle. Les quatre boutons de la barre du bas font la même
chose au clic ou au doigt.

Il n'y a **pas de touche pour quitter** : `prefix+a` referme le pane, geste
symétrique de celui qui l'a ouvert.

## Ouvrir

```toml
# ~/.config/herdr/config.toml
[[keys.command]]
key = "prefix+a"
type = "plugin_action"
command = "herdr-scratchpad.open-scratchpad"
description = "toggle scratchpad"
```

`prefix+a` ouvre le pane docké en bas, le focalise s'il est déjà ouvert, le
ferme s'il est focalisé. Un pane laissé mort par un redémarrage du serveur est
remplacé plutôt que dupliqué.

## Ce qui le distingue de [`herdr-notes`](https://github.com/alexarthurs/herdr-notes)

Notes est un **carnet** : markdown rendu, mode preview/édition, une note par
workspace. Les deux cohabitent très bien.

Ceci est un **presse-papier** :

- **un seul texte, global** — le même dans tous les workspaces et toutes les
  tabs, parce qu'on colle depuis le projet A précisément pour récupérer dans le
  projet B ;
- **toujours éditable** — pas de mode, pas de touche pour passer en écriture ;
- **texte brut** — l'état est un `.txt`, pas un JSON.

## Le canal bidirectionnel

C'est la propriété la plus utile, et elle est gratuite.

L'état vit dans `$HERDR_PLUGIN_STATE_DIR/scratchpad.txt`, en clair. Le fichier
**est** le texte, sans échappement. Donc :

```bash
# lire ce qu'il y a dans le scratchpad
cat ~/.local/state/herdr/plugins/herdr-scratchpad/scratchpad.txt

# y déposer quelque chose depuis n'importe où
git log --oneline -20 > ~/.local/state/herdr/plugins/herdr-scratchpad/scratchpad.txt
```

Le pane surveille le fichier et se recharge tout seul quand il change — sauf
s'il a des frappes non sauvegardées, auquel cas ce que tu tapes gagne. Un agent
qui écrit ce fichier fait apparaître le texte devant toi ; ce que tu colles dans
le pane, il peut le lire.

Plusieurs panes scratchpad peuvent être ouverts en même temps : ils partagent le
fichier, donc le texte.

## Copier, et la limite des 192 Ko

La copie passe par **OSC 52** : le texte remonte au presse-papier du terminal
d'où tu es connecté, pas de la machine où tourne herdr. C'est ce qu'il faut en
SSH, et ça marche sur une machine sans serveur graphique.

herdr plafonne les écritures presse-papier à **192 Ko**
(`MAX_CLIPBOARD_BYTES`). Au-delà, `Ctrl+C` refuse explicitement et te renvoie
vers `Ctrl+S` — plutôt que de te laisser coller du vide ailleurs.

`Shift`+glisser sélectionne normalement, comme dans n'importe quel pane :
herdr réserve `Shift`+souris au terminal, et le plugin n'y touche pas.

## Export

`Ctrl+S` écrit `/tmp/herdr-scratchpad.txt` — chemin fixe, écrasé, affiché
3 secondes dans la barre.

Ce n'est pas « sortir le texte » (le fichier d'état s'en charge déjà) : c'est
**figer un instantané**. Le fichier d'état bouge tout seul ; celui-là non.

## Construire

```
cargo build --release
cargo test
cargo clippy --all-targets -- -D warnings
```

Rust + ratatui, aucune dépendance système. Linux et macOS.

## Licence

MIT.
