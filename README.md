# herdr-scratchpad

**Le presse-papier qui se souvient.**

Un panneau [herdr](https://herdr.dev) docké en bas où l'on colle, où l'on
récupère, et qu'on vide. Pas un carnet : un outil de **transit**. Le texte est
sauvegardé tout seul, en texte brut, et revient après un redémarrage.

**Un buffer par tab** : le scratchpad d'une tab a son texte et parle aux agents
de cette tab, pas à ceux d'à côté. Ce qu'on voit, ce qu'on édite et ce à quoi
on parle coïncident.

```
herdr plugin link .
```

## Les touches

| Touche | Bouton | Action |
| --- | --- | --- |
| `Ctrl+E` | `^E envoyer` | dépose le texte chez un agent, vide, et bascule dessus |
| `Ctrl+N` | `→ claude·p3` | agent suivant de la tab (à partir de deux) |
| `Ctrl+C` | `^C copier` | copie tout vers le presse-papier de ta machine |
| `Ctrl+L` | `^L vider` | vide, sans confirmation |
| `Ctrl+S` | — | écrit `/tmp/herdr-scratchpad-$HERDR_TAB_ID.txt` |
| `Ctrl+Z` | `^Z annuler` | ramène le dernier contenu vidé ou envoyé |

Ce sont les combinaisons que tes doigts connaissent déjà, chacune à sa
signification habituelle. Les boutons de la barre du bas font la même chose au
clic ou au doigt — sauf `Ctrl+S`, qui n'a pas de bouton : il dépose un fichier
qu'on ira lire ailleurs, ce n'est pas un geste au doigt.

Il n'y a **pas de touche pour quitter** : `prefix+a` referme le pane, geste
symétrique de celui qui l'a ouvert — et c'est aussi lui qui le rouvre depuis
l'agent où `Ctrl+E` vient de te poser. Un `Ctrl+Q` sur un panneau qu'on veut
permanent ne sert qu'à le fermer par erreur.

## Envoyer à un agent

`Ctrl+E` **dépose** le texte dans la boîte de saisie d'un agent herdr. Il ne
l'envoie pas : **aucune Entrée n'est tapée**. Tu bascules sur l'agent, tu relis,
tu soumets toi-même — ou tu effaces. Déposer chez un agent occupé est sans
danger, le texte attend dans la boîte.

Un prompt de dix lignes arrive comme **un seul collage**, pas ligne par ligne :
herdr l'enveloppe dans un *bracketed paste*.

Une fois le dépôt confirmé, le scratchpad se vide et **le focus passe sur
l'agent** — tu atterris devant ton texte, prêt à le relire et à l'envoyer. Le
vidage est un *déplacement*, pas une copie : `Ctrl+Z` le rattrape comme
n'importe quel vidage, en revenant par `prefix+a`. **Si l'envoi échoue, rien
n'est vidé et rien ne bascule** : c'est ce qui rend l'erreur sans conséquence.

La destination est affichée en permanence à côté du bouton, `→ claude·wdv` :
l'agent, puis le workspace. C'est le garde-fou, et il remplace toute
confirmation — on ne doit jamais appuyer sans pouvoir lire où ça part. `Ctrl+N`
ou un clic sur la zone passent à l'agent suivant.

Par défaut la cible est l'agent de la **même tab que le pane** — celui d'où tu
viens d'ouvrir le scratchpad. À défaut un agent du même workspace, à défaut la
dernière cible utilisée, à défaut le premier venu. Sans agent du tout, la zone
affiche `→ aucun agent` et le bouton refuse.

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

L'état vit dans `$HERDR_PLUGIN_STATE_DIR/scratchpad-$HERDR_TAB_ID.txt`, en
clair. Le fichier **est** le texte, sans échappement. `HERDR_TAB_ID` est présent
dans le pane de l'agent comme dans les autres : il compose le chemin lui-même,
sans qu'on le lui donne. Donc, depuis le pane d'un agent :

```bash
D=~/.local/state/herdr/plugins/herdr-scratchpad

# lire ce qu'il y a dans le scratchpad de ma tab
cat "$D/scratchpad-$HERDR_TAB_ID.txt"

# y déposer quelque chose
git log --oneline -20 > "$D/scratchpad-$HERDR_TAB_ID.txt"
```

Le pane surveille le fichier et se recharge tout seul quand il change — sauf
s'il a des frappes non sauvegardées, auquel cas ce que tu tapes gagne. Un agent
qui écrit ce fichier fait apparaître le texte devant toi ; ce que tu colles dans
le pane, il peut le lire.

Plusieurs panes scratchpad peuvent être ouverts en même temps, un par tab :
chacun a son fichier, donc son texte. Pour faire passer du texte d'une tab à
l'autre, `Ctrl+C` ou `Ctrl+S` — un geste explicite.

La clé est figée à l'ouverture du pane : un scratchpad déplacé vers une autre
tab (`pane move`) garde le buffer et les cibles de sa tab d'origine.

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

`Ctrl+S` écrit `/tmp/herdr-scratchpad-$HERDR_TAB_ID.txt` — chemin fixe pour une
tab, écrasé, affiché 3 secondes dans la barre. Une tab n'écrase donc jamais
l'instantané d'une autre.

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
