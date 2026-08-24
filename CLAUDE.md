# CLAUDE.md

Notes de développement pour `herdr-scratchpad`. Le *quoi* et le *pourquoi* des
décisions sont dans [`DESIGN.md`](DESIGN.md) ; ici, le *comment* et les pièges.

## Langue

Le dépôt est rédigé **en français**, code compris (commentaires, messages
d'interface, noms de tests). Répondre et rédiger en français.

## Boucle de travail

```
cargo build --release
cargo test
cargo clippy --all-targets -- -D warnings
```

Les trois au vert avant de livrer. La compilation initiale prend ~60 s sur un
Raspberry Pi 4 ; les suivantes ~8 s.

Pour essayer en vrai — **fermer et rouvrir le pane après chaque
`cargo build --release`** : un scratchpad déjà ouvert continue de tourner sur
l'ancien binaire, et on teste alors le comportement qu'on vient de corriger
(vérifiable par `ps -o lstart=` contre le mtime du binaire) :

```
herdr plugin link .
herdr plugin action invoke herdr-scratchpad.open-scratchpad
herdr pane read <pane_id>          # lire l'écran du pane, très utile en test
herdr pane send-keys <pane_id> ctrl+s
```

## Architecture

| Fichier | Rôle |
| --- | --- |
| `src/main.rs` | deux vies du binaire : la TUI, ou un mode stdin→stdout pour le lanceur |
| `src/app.rs` | état, touches, souris, horloges |
| `src/agents.rs` | cibles d'envoi : croisement JSON, préférence, cyclage, libellé |
| `src/buffer.rs` | texte et curseur, édition minimale |
| `src/ui.rs` | rendu, repliage, géométrie des boutons |
| `src/state.rs` | persistance texte brut, écriture atomique, surveillance mtime |
| `src/clipboard.rs` | OSC 52 et base64 |
| `src/launch.rs` | décisions du toggle, en fonctions pures |
| `src/ipc.rs` | client socket : estampille, `agent.list`, `workspace.list`, dépôt |
| `scripts/open-scratchpad.sh` | enchaînement de commandes herdr, aucune décision |

**Règle** : le script ne décide rien. Toute logique vit dans `launch.rs`, en
fonctions pures testables (`--launch-decision`, `--focused-pane`,
`--open-plan`). Un bug de toggle se reproduit avec un `echo | binaire`, jamais
en réouvrant des panes à la main.

## Pièges herdr (vérifiés dans les sources, pas devinés)

### Le manifeste ne sait pas docker en bas

`placement` n'accepte que `overlay|popup|split|tab|zoomed`. Il n'y a **ni
`direction` ni `ratio`** dans `[[panes]]`, et un split déclaratif est câblé en
50/50 vers la droite (`ratio.unwrap_or(0.5)`, herdr `src/workspace/tab.rs:361`).
Le dock bas à 30 % passe obligatoirement par `pane split --direction down
--ratio` dans le lanceur. L'entrée `[[panes]]` est un repli dégradé.

### `--ratio` est la part de la CIBLE

`--direction down --ratio 0.70` laisse 70 % au pane d'origine (en haut) et 30 %
au nouveau (en bas). herdr écrête à `0.1..=0.9`. Scinder le pane le **plus
bas** vers le bas le pose contre le bord : aucun échange de panes nécessaire.

### `pane split` n'hérite pas de `HERDR_PLUGIN_STATE_DIR`

Les **actions** le reçoivent, les panes créés par `pane split` non. Sans
`--env "HERDR_PLUGIN_STATE_DIR=$HERDR_PLUGIN_STATE_DIR"`, la TUI écrit son état
ailleurs que le reste du plugin. Un `[[panes]]` déclaratif, lui, le reçoit
nativement.

En revanche `HERDR_PANE_ID`, `HERDR_TAB_ID` et `HERDR_WORKSPACE_ID` **sont**
injectés dans tout pane que herdr crée, `pane split` compris (`herdr
src/pane.rs:145`, vérifié dans `/proc/<pid>/environ` du pane). Ne pas les
repasser en `--env` : ceux du lanceur désigneraient le pane *appelant*.

### `pane run` ne lance pas un processus

Il **tape** la commande dans le shell du pane, suivie d'Entrée, **sans
échappement** (`herdr src/cli/pane.rs:1047`). D'où le `exec "$bin"` : `exec`
remplace le shell, donc le pane meurt avec la TUI.

### La cible d'envoi par défaut se lit dans la **tab**

`HERDR_WORKSPACE_ID` ne suffit pas : un workspace porte plusieurs tabs, donc
plusieurs agents, et « l'agent du pane courant » est celui de la tab — le
scratchpad naît d'un split du pane focalisé et hérite de sa tab. Le repli sur le
workspace ne sert qu'au scratchpad seul dans sa tab.

### `keys: []` dépose, il ne soumet pas

`pane.send_input` écrit le texte dans la boîte de saisie du pane visé, puis tape
les touches de `keys`. Une liste **vide** est donc tout le contrat de l'envoi
(§14 du DESIGN) : le texte attend, l'utilisateur soumet lui-même. Ajouter
`"Enter"` à cette liste changerait la fonctionnalité, pas son confort.

`agent.prompt` fait l'inverse (il soumet) — ne pas le substituer « pour
simplifier ».

L'écriture est un **ajout au curseur**, pas un remplacement : ce que l'agent
avait déjà dans sa boîte reste devant (`herdr src/app/api_helpers.rs:69`).

### Le multiligne n'a pas besoin d'être découpé

herdr enveloppe le texte dans un *bracketed paste* dès que le pane cible a
activé `?2004h` (`herdr src/pane.rs:2858`), ce que fait Claude Code. Dix lignes
arrivent comme **un seul collage** et ne se font pas soumettre à la première.

Ne pas réintroduire de découpage ligne à ligne : ce serait exactement le bug
qu'on croit corriger.

### Une erreur socket arrive avec un transport parfaitement sain

`{"id":…,"error":{"code":…,"message":…}}` sur la même connexion qu'un succès.
D'où `ipc::error_of` : un envoi qui « marche » au niveau du tuyau peut avoir
échoué, et c'est cet échec qui empêche le scratchpad de se vider. Le vérifier,
pas le supposer.

### L'estampille doit tomber entre `split` et `run`

Un jeton absent vaut « mort » (cf. plus bas). Si la TUI était la première à
estampiller, un second toggle pendant son démarrage prendrait le pane neuf pour
un cadavre et le remplacerait — en boucle. Le `--stamp` synchrone du lanceur
comble exactement cette fenêtre.

### Jeton absent = cadavre, pas « frais »

Un redémarrage du serveur herdr restaure l'étiquette et le scrollback d'un pane,
mais ni son processus ni ses jetons, et n'émet aucun événement. Un pane étiqueté
`Scratchpad` sans jeton est donc une coquille vide : le traiter comme périmé le
fait remplacer au lieu d'être zoomé indéfiniment.

La valeur du jeton **doit être une chaîne** ; herdr rejette les nombres avec
`invalid_request`.

### Pas de « focaliser tel pane »

`herdr pane focus` est directionnel. Le cycle `pane zoom <id> --on` puis
`--off` est le contournement — c'est celui du lanceur, qui n'a que la CLI.

Depuis le socket, en revanche, `agent.focus` fait le travail en un appel quand
la cible est un **agent** : il accepte le `pane_id` public et le résout en
premier (`herdr src/app/terminal_targets.rs:79`), puis appelle
`switch_workspace_tab` — donc il bascule tab et workspace au besoin
(`src/app/agents.rs:75`). Passer le `pane_id` et non le nom : `claude` serait
ambigu dès qu'il y a deux sessions.

### `pane close` tue sans signal

D'où la séquence de fermeture : déplacer le focus (`pane focus --direction
up`), attendre 0,4 s, puis fermer. Perdre le focus déclenche une sauvegarde
immédiate — herdr transmet l'événement aux panes qui demandent `?1004h`
(`herdr src/pane.rs:2867`).

Limite connue : un scratchpad **seul dans sa tab** n'a nulle part où céder le
focus, donc la fenêtre de temporisation (500 ms) subsiste dans ce cas précis.

### Re-lister après une fermeture

Un instantané de `pane list` périme dès qu'un pane disparaît, et le cadavre
pouvait être le pane focalisé. Sans re-liste, `pane layout`/`pane split`
échouent en `pane_not_found`.

### `pane list` sans `--workspace`

La liste globale est la seule où figure le pane réellement focalisé. La scoper
avec l'id de workspace du shell lanceur — figé au spawn — peut faire disparaître
le pane focalisé et dégrader la décision en `OPEN`, donc en doublon.

## Pièges terminal

### `ratatui::init()` ne suffit pas

Il pose le mode brut et l'écran alterné, rien d'autre. Il faut activer à la main
`EnableMouseCapture`, `EnableBracketedPaste` et `EnableFocusChange` — et
**chaîner un hook de panique devant celui de ratatui** pour les désactiver,
sinon une panique laisse le terminal de l'utilisateur bloqué en mode
rapport-souris.

### `Shift`+souris appartient au terminal

herdr y réserve la sélection native. Tout gestionnaire de souris doit sortir
immédiatement si `SHIFT` est présent. C'est ce qui permet de garder la sélection
au glisser malgré la capture.

### AltGr arrive en `CONTROL | ALT`

Sur Windows, taper `@` ou `#` déclencherait une commande sans la garde
`altgr` de `App::on_key`. Le plugin ne cible pas Windows aujourd'hui, mais la
garde ne coûte rien et évite un bug silencieux le jour où ça change.

### Les horloges se pompent à chaque tour de boucle

Pas seulement quand l'attente expire. Une saisie soutenue (répétition de touche,
long collage) affamerait sinon l'estampille jusqu'à ce que le lanceur déclare le
pane mort et le remplace en pleine frappe.

### Compter en colonnes d'affichage, pas en caractères

Un idéogramme occupe deux colonnes. Le repliage et la position du curseur
utilisent `UnicodeWidthChar::width`, jamais `chars().count()`. Et les insertions
dans une `String` sont en **octets** : d'où `byte_offset` dans `buffer.rs`.

### Plusieurs écrivains sur le fichier d'état

Le design autorise plusieurs panes ouverts. Le fichier temporaire de l'écriture
atomique porte donc le **pid** — un nom fixe (comme celui de `herdr-notes`) les
ferait se piétiner.

## Limites du presse-papier

`MAX_CLIPBOARD_BYTES = 192 * 1024` (herdr `src/ghostty/mod.rs:468`), testé sur
les octets **décodés** (`:612`). Un payload vide est rejeté en `UNSUPPORTED` :
`clipboard::check` refuse donc les deux cas en amont, pour pouvoir afficher un
message au lieu de laisser l'utilisateur coller du vide.

La séquence utilise le terminateur **BEL** et non ST : certains terminaux
n'honorent que celui-là, et c'est la forme que herdr émet lui-même
(`src/selection.rs:352`).

## Ce qui n'est délibérément pas là

Ne pas ajouter sans rouvrir `DESIGN.md` :

- readline (`Ctrl+A/E/K/W/U`) — ces lettres sont réservées aux commandes,
  `Ctrl+E` est devenue « envoyer » ;
- undo de frappe — `Ctrl+Z` appartient au rattrapage du vidage ;
- confirmation au vidage — la case de secours est la réponse ;
- markdown rendu, mode preview — c'est `herdr-notes` ;
- cloisonnement par workspace — le buffer est global, c'est le point ;
- touche pour quitter — `prefix+a` referme ;
- confirmation à l'envoi — la cible affichée en permanence *est* le garde-fou ;
- message de retour au cyclage — il masquerait la zone cible pendant 3 s, donc
  le clic suivant ; la zone est son propre retour ;
- traitement du cas « agent occupé » — sans objet quand on dépose sans soumettre ;
- garde-fou de taille à l'envoi — le plafond est 1 Mo, inatteignable ;
- Windows — intestable depuis un Pi headless.
