# Plan — bouton « envoyer à l'agent »

> Plan d'implémentation autonome. Il est écrit pour être exécuté dans une
> **session neuve**, sans le contexte de la conversation qui l'a produit.
> Tout ce qui est nécessaire est ici ou référencé par chemin exact.

## 0. Orientation (à lire en premier, 5 minutes)

| Fichier | Pourquoi |
| --- | --- |
| `DESIGN.md` | les décisions de conception du plugin et **leurs raisons** — ne rien contredire sans instruction explicite |
| `CLAUDE.md` | la boucle de travail et les pièges herdr déjà payés |
| `src/app.rs` | c'est là que 80 % du travail se fait |
| `src/ui.rs` | barre de boutons et géométrie de clic |
| `src/ipc.rs` | client socket, à étendre |

Sources de herdr disponibles localement pour vérification :
`~/workspace/github.com/herdrdev/herdr`.

**Langue du dépôt : français**, code et commentaires compris.

Boucle de travail :

```
cargo build --release && cargo test && cargo clippy --all-targets -- -D warnings
```

## 1. Ce qu'on ajoute, en une phrase

Un bouton **`[^E envoyer]`** et une zone **`[→ claude·wdv]`** en tête de la barre
du bas : le texte du scratchpad est **déposé** dans la boîte de saisie d'un
agent herdr — sans Entrée — puis le scratchpad se vide dans sa case de secours.

## 2. Décisions arrêtées

Elles ont été tranchées une par une. Les rouvrir demande une instruction
explicite.

### 2.1 Déposer, pas soumettre

`pane.send_input` avec `keys: []`. Le texte atterrit dans le champ de saisie de
l'agent ; **on n'envoie pas Entrée**. L'utilisateur bascule, relit, soumet
lui-même.

`agent.prompt` existe et soumettrait directement — **ne pas l'utiliser**. Le
choix est délibéré : l'envoi devient une action réversible côté agent.

Effet de bord bienvenu : déposer chez un agent **occupé** est inoffensif, le
texte attend dans la boîte. Il n'y a donc **aucun cas « agent en train de
travailler » à traiter**.

### 2.2 Le scratchpad se vide après un dépôt réussi

Même chemin que `Ctrl+L` : le texte part dans la case de secours à une place,
`Ctrl+Z` le rattrape. Le dépôt est un *déplacement*, pas une copie — un outil
de transit dont le contenu survit à son usage se transforme en carnet par
accumulation.

**En cas d'échec, on ne vide pas.** C'est ce qui rend l'erreur sans conséquence.

### 2.3 La cible est affichée en permanence

C'est le garde-fou principal, et il remplace toute confirmation modale.
« Envoyer » est la seule commande **sortante** du plugin : les quatre autres
restent chez l'utilisateur, celle-ci démarre du travail ailleurs. Un bouton
dont on ne peut pas lire la destination avant d'appuyer est un piège — surtout
avec plusieurs agents `claude` que rien ne distingue.

Libellé : `→ <agent>·<label du workspace>`, par exemple `→ claude·wdv`. Rogné
par la droite quand la barre est serrée ; **le nom de l'agent survit au
workspace**.

### 2.4 Choix de la cible par défaut, dans cet ordre

1. l'agent du **même workspace que ce pane** (`HERDR_WORKSPACE_ID`, injecté par
   herdr dans tout pane) ;
2. sinon la **dernière cible utilisée par ce scratchpad**, mémorisée sur disque ;
3. sinon le **premier agent disponible** ;
4. sinon aucune : la zone affiche `→ aucun agent` et le bouton refuse au clic.

### 2.5 Cyclage

Clic sur la zone cible, **ou** `Ctrl+N` (« agent suivant »). Les deux voies
existent parce que la barre existe pour être utilisable au doigt sur petit
écran : une commande dangereuse ne doit pas être la seule à exiger le clavier.

`Tab` a été écarté : dans une zone de texte il doit insérer une tabulation, et
un scratchpad qui mange les tabulations d'un bloc de code collé trahit sa
fonction.

### 2.6 Fraîcheur de la cible

- rafraîchissement de l'affichage toutes les **2,5 s** ;
- **revérification au moment de l'envoi** : la cible est re-résolue juste avant
  d'agir. Si elle a disparu → message, rien n'est envoyé, rien n'est vidé.

L'affichage n'a besoin d'être qu'à peu près à jour ; l'action doit être
exactement juste.

### 2.7 Placement dans la barre, et ordre de rognage

Nouvelle barre, de gauche à droite :

```
[^E envoyer] [→ claude·wdv] [^C copier] [^L vider] [^S fichier] [^Z annuler]
```

`envoyer` et sa cible passent **en tête**. Le rognage abandonne les boutons qui
ne tiennent pas **en partant de la droite** (comportement actuel de
`ui::button_rects`, conservé) : sur un pane étroit c'est donc `^Z annuler` qui
disparaît en premier — il a déjà sa touche.

### 2.8 La mémoire de la cible ne va PAS dans `scratchpad.txt`

Ce fichier doit rester du **texte nu** : c'est tout le contrat du canal
bidirectionnel (`DESIGN.md` §8). La dernière cible va dans un fichier voisin
`target.txt`, au même endroit.

Contenu : `<label du workspace>\t<agent>` — **pas** le `pane_id`, qui ne survit
pas à un redémarrage de herdr. La résolution au démarrage re-cherche un agent
correspondant à cette paire.

## 3. Faits vérifiés (ne pas re-découvrir)

Tous vérifiés dans les sources de herdr et contre le binaire installé (0.8.2).

### Méthodes socket

| Méthode | Usage ici |
| --- | --- |
| `agent.list` | lister les agents (`herdr src/api/schema.rs:106`) |
| `workspace.list` | obtenir le **label** des workspaces (`:68`) |
| `pane.send_input` | déposer le texte (`:172`) |

`ipc.rs` parle déjà ce protocole : JSON délimité par des sauts de ligne, **une
requête par connexion**, forme
`{"id": "...", "method": "...", "params": {...}}`.

### Paramètres du dépôt

`PaneSendInputParams` (`herdr src/api/schema/panes.rs:266`) :

```rust
pub struct PaneSendInputParams {
    pub pane_id: String,
    pub text: String,
    pub keys: Vec<String>,   // <- VIDE pour déposer sans Entrée
}
```

### Forme des réponses

`agent.list` → `result.agents[]`, champs utiles :

```json
{"agent":"claude","agent_status":"idle","focused":false,
 "pane_id":"w2:p1","tab_id":"w2:t1","workspace_id":"w2",
 "terminal_title_stripped":"Questionnaire e-commerce"}
```

`workspace.list` → `result.workspaces[]` :

```json
{"workspace_id":"w2","label":"wdv","number":2,"focused":false}
```

### Le multiligne est sûr

herdr enveloppe le texte envoyé à un pane dans un *bracketed paste* dès que ce
pane a activé `?2004h` (`herdr src/pane.rs:2858`) — et Claude Code l'active. Un
prompt de dix lignes arrive donc comme **un seul collage** et ne se fait pas
soumettre à la première ligne.

**C'était le risque principal de la fonctionnalité ; il n'existe pas.** Ne pas
inventer de découpage ligne à ligne.

### Plafond de taille

`MAX_INPUT_PAYLOAD = 1 MB` (`herdr src/server/client_transport.rs:41`), soit
cinq fois le plafond presse-papier de 192 Ko. En pratique inatteignable pour un
prompt : **ne pas ajouter de garde-fou de taille** sans preuve qu'il serve.

## 4. Travail, fichier par fichier

### 4.1 `src/ipc.rs` — lire les réponses

Aujourd'hui `call()` envoie et **jette** la réponse. Il faut une variante qui la
rende.

- extraire `fn call_json(method, params) -> Option<serde_json::Value>` qui lit
  la ligne de réponse et la parse ;
- réécrire `call()` comme un appel à `call_json` dont on ignore le résultat, et
  `stamp()` reste inchangé côté appelant ;
- ajouter `pub fn send_input(pane_id: &str, text: &str) -> Result<(), String>` :
  `pane.send_input` avec `keys: []`, et **remonter l'erreur** (contrairement à
  `stamp`, ici un échec doit être visible — c'est lui qui empêche le vidage) ;
- ajouter `pub fn agent_list() -> Option<Value>` et
  `pub fn workspace_list() -> Option<Value>`.

Garder la discipline du module : hors herdr, tout doit dégrader silencieusement
plutôt que paniquer.

### 4.2 `src/agents.rs` — nouveau module

Fonctions **pures** sur du JSON, testables sans socket (même discipline que
`launch.rs`, qui est le modèle à imiter).

```rust
pub struct Target {
    pub pane_id: String,
    pub agent: String,          // "claude"
    pub workspace_id: String,
    pub workspace_label: String,
}

/// Croise agent.list et workspace.list en cibles ordonnées de façon stable.
pub fn targets(agents_json: &str, workspaces_json: &str) -> Vec<Target>;

/// Applique l'ordre de préférence de §2.4.
pub fn pick_default(
    targets: &[Target],
    current_workspace: Option<&str>,   // HERDR_WORKSPACE_ID
    remembered: Option<(&str, &str)>,  // (label workspace, agent)
) -> Option<usize>;

/// Cible suivante, en boucle.
pub fn next(targets: &[Target], current: Option<usize>) -> Option<usize>;

/// Libellé de la zone, rogné à `width` colonnes, l'agent survivant au workspace.
pub fn label(target: Option<&Target>, width: usize) -> String;
```

Points d'attention :

- **ordre stable** : trier par `workspace_id` puis `pane_id`, sinon le cyclage
  saute d'un rafraîchissement à l'autre ;
- **exclure le pane courant** s'il apparaissait comme agent (il ne devrait pas,
  mais un garde-fou coûte une ligne) ;
- un workspace sans label connu retombe sur son `workspace_id`.

### 4.3 `src/state.rs` — mémoire de la cible

```rust
pub fn load_target() -> Option<(String, String)>;   // (label workspace, agent)
pub fn save_target(workspace_label: &str, agent: &str);
```

Fichier `target.txt`, même répertoire que `scratchpad.txt`, même écriture
atomique (le temporaire porte déjà le pid, ne pas y toucher). Échec = silence :
perdre la mémoire de la cible n'est pas grave.

**Ne pas toucher au format de `scratchpad.txt`.**

### 4.4 `src/ui.rs` — barre dynamique

C'est le vrai refactor. Aujourd'hui `BUTTONS` est une constante et `draw`
indexe `BUTTONS[i].1` en parallèle des rectangles rendus par `button_rects` :
les deux sont alignés **par coïncidence**. Avec des libellés dynamiques, cette
fragilité devient un bug.

- remplacer la constante par une construction de `Vec<(Action, String)>` où
  `Action` couvre les commandes **et** `CycleTarget` ;
- `button_rects` prend cette liste et rend `Vec<(Action, Rect)>` ;
- `draw` dessine **depuis la même liste**, jamais depuis un index parallèle ;
- conserver la règle « un bouton qui ne tient pas entièrement n'est pas
  enregistré » — c'est elle qui évite les cibles cliquables invisibles ;
- la zone cible se distingue visuellement du bouton d'envoi (style différent) :
  l'une agit, l'autre informe et sélectionne.

Les tests existants de `ui.rs` sur l'ordre et le rognage doivent être mis à jour,
pas supprimés.

### 4.5 `src/app.rs` — état et commandes

- `Command` gagne `Send` ; `Action` distingue `Command(..)` de `CycleTarget`.
- Nouveaux champs : `targets: Vec<Target>`, `target: Option<usize>`,
  `last_targets_refresh: Instant`.
- `const TARGET_REFRESH: Duration = Duration::from_millis(2500);`
- `maybe_refresh_targets()` — pompée dans la boucle comme les autres horloges,
  **à chaque tour** et non seulement à l'expiration de l'attente (piège déjà
  documenté dans `CLAUDE.md`). Préserver la cible sélectionnée à travers un
  rafraîchissement : la retrouver par `pane_id`, sinon re-appliquer `pick_default`.
- Touches : `Ctrl+E` → `Send`, `Ctrl+N` → `CycleTarget`. Elles passent par la
  même garde AltGr que les autres.
- `send()` :
  1. si le buffer est vide → « rien à envoyer », **on s'arrête** ;
  2. re-résoudre les cibles (appel socket) ;
  3. cible absente → « agent introuvable », rien n'est vidé ;
  4. `ipc::send_input(pane_id, texte)` ; erreur → message, rien n'est vidé ;
  5. succès → `save_target(...)`, puis **le même chemin que `clear()`** (case de
     secours + vidage + sauvegarde immédiate) ;
  6. message : `envoyé → claude·wdv`.

Ne pas dupliquer la logique de vidage : appeler le chemin existant.

### 4.6 Documentation

- `DESIGN.md` : ajouter un **§14 « Envoyer à l'agent »** reprenant §2 de ce plan,
  y compris les options écartées et pourquoi.
- `README.md` : la nouvelle ligne dans le tableau des touches, et un court
  paragraphe sur le dépôt (insister sur « sans Entrée »).
- `CLAUDE.md` : ajouter aux pièges — `keys: []` pour déposer sans soumettre, et
  le fait que le bracketed paste rend le multiligne sûr.

## 5. Tests à écrire

Suivre le style existant : noms de tests en français, une assertion par
comportement, aucun accès disque ni socket dans les tests unitaires.

**`agents.rs`** (le gros du filet) :

- une liste vide ne rend aucune cible ;
- un agent sans workspace connu retombe sur son `workspace_id` comme libellé ;
- l'ordre est stable entre deux appels sur la même entrée ;
- `pick_default` préfère l'agent du workspace courant ;
- à défaut, il retrouve la cible mémorisée par (label, agent) ;
- à défaut encore, il prend le premier ;
- il rend `None` quand la liste est vide ;
- `next` boucle et revient au début ;
- `next` sur une liste vide rend `None` ;
- `label` rogne le workspace avant le nom de l'agent ;
- `label(None, …)` rend `aucun agent` ;
- du JSON illisible ne panique pas et rend une liste vide.

**`app.rs`** :

- `Ctrl+E` sur un buffer vide ne vide rien et le dit ;
- un envoi qui échoue **ne vide pas** le buffer ;
- un envoi réussi vide, et `Ctrl+Z` restaure ;
- `Ctrl+N` fait tourner la cible sans toucher au texte ;
- un clic sur la zone cible cycle, un clic sur `^E` envoie ;
- `Shift`+clic reste inerte (règle générale du plugin).

Pour les cas socket, introduire une **couture d'injection** (un champ de type
fonction, ou un petit trait) plutôt que d'appeler le vrai `ipc` : `App::headless`
existe déjà pour ce genre d'isolement, s'en inspirer.

**`ui.rs`** : mettre à jour l'ordre attendu et vérifier que sur une barre étroite
`envoyer` et la cible **survivent** au rognage.

## 6. Recette manuelle

Après `cargo build --release` :

```bash
H=~/.local/bin/herdr

# ouvrir le scratchpad
$H plugin action invoke herdr-scratchpad.open-scratchpad
$H pane list | python3 -c "import sys,json;print([p['pane_id'] for p in json.load(sys.stdin)['result']['panes'] if p.get('label')=='Scratchpad'])"

SP=<pane_id du scratchpad>

# y mettre du texte multiligne par le fichier d'état
printf 'ligne une\nligne deux\nligne trois' > ~/.local/state/herdr/plugins/herdr-scratchpad/scratchpad.txt
sleep 2

# lire la barre : elle doit montrer [^E envoyer] [→ claude·<workspace>]
$H pane read "$SP" | tail -c 200

# cycler, puis envoyer
$H pane send-keys "$SP" ctrl+n
$H pane send-keys "$SP" ctrl+e

# vérifier : le scratchpad est vide, et l'agent visé a les trois lignes
#            DANS SA BOÎTE DE SAISIE, NON SOUMISES
$H pane read <pane_id de l agent> | tail -c 300
```

À vérifier explicitement :

- les trois lignes arrivent **ensemble** et **non soumises** ;
- le scratchpad est vide, et `ctrl+z` le remplit à nouveau ;
- avec l'agent fermé entre l'affichage et l'envoi, le scratchpad **n'est pas
  vidé** et la barre affiche l'erreur.

Refermer le pane à la fin (`plugin action invoke` à nouveau) et nettoyer
`scratchpad.txt`.

## 7. Ce qu'il ne faut PAS faire

- **`agent.prompt`** — il soumet ; la décision est de déposer (§2.1).
- **envoyer `Enter`** dans `keys` — même raison.
- **découper le texte ligne par ligne** — le bracketed paste s'en charge (§3).
- **vider avant confirmation du dépôt** — l'ordre est : envoyer, puis vider.
- **mettre la cible dans `scratchpad.txt`** — il reste du texte nu (§2.8).
- **mémoriser un `pane_id`** — il ne survit pas à un redémarrage de herdr.
- **ajouter une confirmation modale** — l'affichage de la cible *est* le
  garde-fou ; le plugin n'a aucun mode et n'en gagne pas ici.
- **traiter le cas « agent occupé »** — sans objet en mode dépôt.
- **toucher aux quatre commandes existantes**, à leurs touches ou au format du
  fichier d'état.
