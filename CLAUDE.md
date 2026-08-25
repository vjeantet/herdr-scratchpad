# CLAUDE.md

Notes de développement pour `herdr-scratchpad`. Le *quoi* et le *pourquoi* des
décisions sont dans [`DESIGN.md`](DESIGN.md) ; ici, le *comment* et les pièges.

## Langue

Le dépôt est rédigé **en français** : documentation et commentaires. Répondre
et rédiger en français.

Deux exceptions, l'une et l'autre tournées vers l'extérieur : les **messages
d'interface** sont en anglais (commit `d9e75b8`), et les **noms de tests**
aussi (2026-08-25) — c'est `cargo test` qui les affiche, et ils se lisent comme
des phrases anglaises. Les *messages d'assertion*, eux, restent en français :
ils s'adressent à qui répare, pas à qui utilise.

## Où en est le projet

Tout ce que `DESIGN.md` décrit est implémenté et vérifié sur des panes vivants,
**sauf** ce qui est listé en « Points ouverts » ci-dessous.

| Fonction | Touche | Design |
| --- | --- | --- |
| coller / éditer / recharger depuis le fichier | — | §1, §8, §9, §11 |
| poser le curseur au clic | clic | §Souris |
| sauter / effacer un mot | `Ctrl`+flèches, `Ctrl+Backspace` | §4 |
| aller au début / à la fin du texte | `Ctrl+Home`, `Ctrl+End` | §4 |
| un buffer par tab, ménage des orphelins | — | §8, §9 |
| copier vers le terminal hôte (OSC 52) | `Ctrl+C` | §5 |
| vider, avec case de secours à une place | `Ctrl+L` | §7 |
| exporter un instantané `/tmp/herdr-scratchpad-<tab_id>.txt` | `Ctrl+S` | §10 |
| rattraper le dernier vidage **ou envoi** | `Ctrl+Z` | §7, §14.2 |
| déposer chez un agent, vider, basculer dessus | `Ctrl+E` | §14 |
| agent suivant (à partir de deux dans la tab) | `Ctrl+N` | §14.5 |
| toggle du pane docké en bas | `prefix+a` | §3 |

`docs/plan-envoi-agent.md` (§14) et `docs/plan-buffer-par-tab.md` (§8, §9, §14.4)
sont les plans qui ont produit ces sections. Ils sont **exécutés** : ce sont des
documents d'archive, pas des listes de travaux.

## Points ouverts

Connus, non traités, par ordre de gêne réelle :

1. **`prefix+shift+a` n'existe pas.** `DESIGN.md` §3 annonce une variante « tab
   dédiée plein écran » ; il n'y a qu'une action au manifeste. Soit
   l'implémenter, soit retirer la promesse du design.
2. **Pas de cyclage arrière.** Assumé tant qu'il y a deux ou trois agents.
3. **Un pane déplacé garde la tab de sa naissance.** `HERDR_TAB_ID` est figé au
   spawn, donc un scratchpad passé à une autre tab par `pane move` continue
   d'écrire dans le buffer de sa tab d'origine et d'en cibler les agents.
   Assumé (§9) : résoudre la tab à chaque tour ferait changer le texte sous les
   yeux au milieu d'un geste.

Ce qui a été résolu par le cloisonnement (2026-08-25) : deux agents de même nom
sont maintenant discernables au suffixe de leur `pane_id` (`→ claude·p3`), et
le scratchpad **seul dans sa tab** n'est plus un cas limite mais un
comportement normal et documenté — un bloc-notes local, sans cible. La fenêtre
de 500 ms de l'autosave à la fermeture, elle, subsiste dans ce cas (cf. « `pane
close` tue sans signal »).

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

### Tester sans marcher sur la session de quelqu'un

`plugin action invoke` agit sur le pane **focalisé**, pas sur le tien : si un
scratchpad est déjà ouvert dans la tab focalisée, l'invocation le *focalise* au
lieu d'en créer un. On se retrouve alors à piloter le pane de l'utilisateur en
croyant piloter le sien — c'est arrivé.

Vérifier à qui on parle avant d'envoyer quoi que ce soit :

```
pgrep -f 'target/release/herdr-scratchpad$'
tr '\0' '\n' < /proc/<pid>/environ | grep HERDR_PANE_ID
ps -o lstart= -p <pid>            # à comparer au mtime du binaire
```

`pane read` est sans effet de bord ; `pane send-keys` et `pane close` n'en sont
pas. Le dépôt chez un agent atterrit dans une vraie boîte de saisie : le
nettoyage est `pane send-keys <agent> ctrl+u`, qui défait le collage.

## Architecture

| Fichier | Rôle |
| --- | --- |
| `src/main.rs` | deux vies du binaire : la TUI, ou un mode stdin→stdout pour le lanceur |
| `src/app.rs` | état, touches, souris, horloges |
| `src/agents.rs` | cibles d'envoi : filtre par tab, cyclage, libellé, tabs vivantes |
| `src/buffer.rs` | texte et curseur, édition minimale |
| `src/ui.rs` | rendu, repliage, géométrie des boutons |
| `src/state.rs` | persistance texte brut par tab, écriture atomique, mtime, ménage |
| `src/clipboard.rs` | OSC 52 et base64 |
| `src/launch.rs` | décisions du toggle, en fonctions pures |
| `src/ipc.rs` | client socket : estampille, `agent.list`, `tab.list`, dépôt |
| `scripts/open-scratchpad.sh` | enchaînement de commandes herdr, aucune décision |

**Règle** : le script ne décide rien. Toute logique vit dans `launch.rs`, en
fonctions pures testables (`--launch-decision`, `--focused-pane`,
`--open-plan`). Un bug de toggle se reproduit avec un `echo | binaire`, jamais
en réouvrant des panes à la main.

## Écrire des tests

Tous unitaires, tous dans le fichier qu'ils testent (`cargo test` en donne le
compte). Conventions :

- **noms en anglais**, une phrase qui dit le comportement attendu
  (`a_click_below_the_last_line_falls_back_to_it`) ; les messages d'assertion
  restent en français et disent *pourquoi* ;
- **aucun socket, nulle part** ; et pas de disque non plus, sauf dans
  `state.rs` où c'est précisément l'objet du test — il travaille alors dans un
  répertoire jetable nommé d'après le pid, jamais dans l'état réel ;
- un test qui aurait besoin d'un pane ou d'un serveur est le signe qu'une
  décision devrait être une fonction pure ailleurs ;
- une assertion par comportement, et le message d'assertion dit *pourquoi*.

Les deux coutures qui rendent ça possible :

- **`launch.rs` et `agents.rs` sont purs** : ils prennent du JSON en `&str` et
  rendent une décision. C'est là que va toute logique qui, sinon, exigerait un
  pane ou un serveur.
- **`ipc::Herdr` est un trait**. `App` en tient un `Box<dyn Herdr>` ;
  `App::with_herdr` en substitue un faux dans les tests (`FakeHerdr`, qui
  répond du JSON figé et retient ce qu'on lui a demandé). Un `Rc<FakeHerdr>`
  est lui aussi un `Herdr`, ce qui permet au test de garder un handle sur le
  faux pendant que `App` en possède un exemplaire.

Ajouter un appel socket = ajouter une méthode au trait, pas un appel direct à
`ipc::` depuis `app.rs`.

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

### Tout se lit dans la **tab**, et `HERDR_TAB_ID` est figé au spawn

Le buffer, les cibles d'envoi et le chemin d'export portent tous la clé de la
tab. `HERDR_WORKSPACE_ID` ne sert plus à rien : un workspace porte plusieurs
tabs, donc plusieurs agents sans rapport avec celui qu'on regardait.

`agent.list` porte déjà le `tab_id` de chaque agent : le filtre est une
comparaison de chaînes, pas un appel de plus. Un agent d'une autre tab n'est
**jamais** une cible, même quand la tab n'en a aucun — pas de repli.

La clé vient de l'environnement, donc elle est **figée au spawn** : `pane move`
déplace le pane, pas son `HERDR_TAB_ID`. C'est une limite assumée, pas un bug à
corriger au prochain passage.

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

Le fichier temporaire de l'écriture atomique porte le **pid** — un nom fixe
ferait se piétiner deux écrivains. Depuis le
cloisonnement par tab, c'est devenu une ceinture sans bretelles : un seul
scratchpad par tab, donc un seul écrivain par fichier. Elle couvre encore le
repli sans clé, hors herdr, où plusieurs binaires lancés à la main partagent
bien un `scratchpad.txt`. Ne pas la retirer pour autant.

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
  `Ctrl+E` est devenue « envoyer ». Les sauts de mot (`Ctrl`+flèches,
  `Ctrl+Backspace`, `Ctrl+Home`/`Ctrl+End`, ajoutés le 2026-08-25) ne sont pas
  une entorse : ils ne portent aucune lettre. Toute autre combinaison `Ctrl` est **avalée** dans
  `on_key`, jamais laissée retomber dans le `match` ordinaire — elle y ferait
  *taper* sa lettre ;
- undo de frappe — `Ctrl+Z` appartient au rattrapage du vidage ;
- confirmation au vidage — la case de secours est la réponse ;
- markdown rendu, mode preview — un scratchpad n'est pas un carnet ;
- **buffer global** — renversé le 2026-08-25 : un buffer par tab, c'est le
  point (§9). Le transit entre tabs passe par `Ctrl+C` ou `Ctrl+S` ;
- cloisonnement par *agent* — cycler la cible remplacerait le texte à l'écran ;
- repli de cible sur le workspace quand la tab n'a aucun agent — la destination
  dépendrait d'un état que rien à l'écran n'explique ;
- migration de l'ancien buffer global vers une tab — il est supprimé sèchement ;
- action de manifeste ou mode `--state-path` pour publier le chemin du buffer —
  une ligne de `cat $D/scratchpad-$HERDR_TAB_ID.txt` dans le README suffit ;
- touche pour quitter — `prefix+a` referme ;
- confirmation à l'envoi — la cible affichée en permanence *est* le garde-fou ;
- message de retour au cyclage — il masquerait la zone cible pendant 3 s, donc
  le clic suivant ; la zone est son propre retour ;
- traitement du cas « agent occupé » — sans objet quand on dépose sans soumettre ;
- garde-fou de taille à l'envoi — le plafond est 1 Mo, inatteignable ;
- Windows — intestable depuis un Pi headless.
