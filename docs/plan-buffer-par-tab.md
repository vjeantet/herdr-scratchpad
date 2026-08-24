# Plan — un buffer par tab

Plan d'implémentation autonome. Il est écrit pour être exécuté dans une
**session neuve**, sans le contexte de la conversation qui l'a produit. Tout ce
qui est nécessaire est ici ou référencé par chemin exact.

## 0. Orientation (à lire en premier, 5 minutes)

| Fichier | Pourquoi |
| --- | --- |
| `DESIGN.md` | les décisions et **leurs raisons** — ce plan en renverse plusieurs, listées en §6 |
| `CLAUDE.md` | la boucle de travail et les pièges herdr déjà payés |
| `src/state.rs` | c'est là que la clé du buffer apparaît |
| `src/agents.rs` | sélection de cible : rétrécit fortement |
| `src/ui.rs` | ordre de la barre, à inverser |
| `src/app.rs` | état, horloges, commandes |

Sources de herdr disponibles localement pour vérification :
`~/workspace/github.com/herdrdev/herdr`.

**Langue du dépôt : français**, code et commentaires compris.

Boucle de travail :

```
cargo build --release && cargo test && cargo clippy --all-targets -- -D warnings
```

## 1. Ce qu'on change, en une phrase

Le buffer cesse d'être **global** et devient **un par tab** :
`scratchpad-<tab_id>.txt`. La cible d'envoi cesse d'être **globale** et devient
**les agents de ma tab**. Tout le reste — coller, éditer, copier, vider,
rattraper, exporter, déposer — ne bouge pas.

## 2. Pourquoi, en trois phrases

Le scratchpad naît d'un `pane split` du pane focalisé : il vit donc dans la
**tab** de l'agent qu'on regardait. Un texte partagé entre toutes les tabs
oblige alors à se demander « d'où vient ce que je vois » et « à qui part ce que
j'envoie » à chaque geste, ce que ni l'écran ni la barre ne peuvent répondre.
Cloisonner par tab fait coïncider ce qu'on voit, ce qu'on édite et ce à quoi on
parle.

## 3. Décisions arrêtées

Tranchées une par une. Les rouvrir demande une instruction explicite. Le numéro
entre parenthèses renvoie à la section de `DESIGN.md` réécrite en §6.

### 3.1 La clé du buffer est la **tab**, pas l'agent (§9)

Un scratchpad = un buffer. Deux agents dans la même tab **partagent** ce buffer
et ne changent que la destination.

La variante « un buffer par agent » a été écartée : cycler la cible
remplacerait le texte à l'écran, c'est-à-dire ferait disparaître ce qu'on vient
de taper au moment précis où l'on vérifie où l'on parle. Dans le cas normal —
un agent, une tab — les deux variantes sont identiques ; elles ne divergent que
dans le cas rare, et là seule celle-ci est prévisible.

### 3.2 Le fichier : `scratchpad-<tab_id>.txt` (§8)

Même répertoire, même écriture atomique, même surveillance mtime. Seul le nom
change.

`HERDR_TAB_ID` est injecté par herdr dans tout pane qu'il crée, `pane split`
compris. Le canal bidirectionnel de §8 survit sans négociation : l'agent
compose lui-même son chemin (§3.9).

### 3.3 Un pane déplacé vers une autre tab garde son buffer (§9)

`HERDR_TAB_ID` est une variable d'environnement **figée au spawn**, et
`pane.move` permet de déplacer un pane vers une autre tab
(`herdr src/api/schema/panes.rs:82`). Un scratchpad déplacé continue donc
d'écrire dans le buffer de sa tab d'origine et de cibler les agents de sa tab
d'origine.

**Assumé, et documenté comme limite connue.** La résolution dynamique — un
`pane.get` sur son propre `HERDR_PANE_ID` à chaque rafraîchissement — a été
examinée et écartée : elle ferait changer le texte sous les yeux au milieu d'un
geste, pour un déplacement qui ne se fait pas.

### 3.4 Hors herdr : `scratchpad.txt` sans suffixe (§8)

Sans `HERDR_TAB_ID`, il n'y a pas de clé. Le binaire doit rester utilisable à
la main : il retombe sur le nom nu, dans le répertoire de repli déjà prévu par
`state_dir()`. Ce fichier ne correspond pas au motif du ménage (§3.6) et n'est
donc jamais supprimé.

### 3.5 L'ancien buffer global est supprimé, sèchement (§8)

Au démarrage : `scratchpad.txt` du **state dir principal** (celui de herdr, pas
celui du repli), plus les **deux** `target.txt` — celui du state dir et celui
de `~/.config/herdr/scratchpad/`, tous deux présents sur la machine de
référence. Pas de sauvegarde, pas de migration vers une tab : adopter l'ancien
texte dans une tab choisie arbitrairement recréerait exactement la surprise
qu'on supprime.

### 3.6 Ménage des orphelins, au démarrage (§8)

Les numéros de tab publics ne sont **jamais réutilisés** (voir §4), donc un
fichier dont la tab n'existe plus est orphelin **définitivement**. Règle :

1. un `tab.list` **non scopé** (toutes les tabs de tous les workspaces) ;
2. **abstention totale** si la réponse est en erreur ou si la liste est vide —
   une liste vide n'est jamais une information, c'est une panne ;
3. n'examiner que les fichiers au motif `scratchpad-w*:t*.txt` ;
4. ne jamais toucher au sien, même si sa tab manquait de la liste ;
5. balayer aussi le répertoire de repli `~/.config/herdr/scratchpad/` ;
6. un échec de ménage est **muet** : un message d'erreur au démarrage pour une
   corvée ratée serait du bruit.

Un buffer vidé laisse un fichier vide ; c'est le ménage qui l'emporte quand sa
tab meurt, pas la sauvegarde.

### 3.7 La cible : les agents de **ma tab**, point (§14.4)

Disparaissent avec cette décision :

- le repli sur le workspace, et le repli « premier agent disponible » ;
- `target.txt`, `state::load_target`, `state::save_target` ;
- `agents::pick_default` ;
- l'appel `workspace.list` du rafraîchissement, et le libellé de workspace.

Un scratchpad **seul dans sa tab** n'a donc aucune cible, jamais, quoi qu'il
arrive ailleurs. C'est un bloc-notes local — assumé. L'exception « si ma tab
n'a aucun agent, alors on rouvre au workspace » a été écartée : elle rend la
destination dépendante d'un état que rien à l'écran n'explique.

Cible par défaut : la **première par `pane_id`** (l'ordre est déjà stable). Le
cyclage vit **en mémoire seule** et se perd à la fermeture du pane.

### 3.8 Libellé et cyclage (§14.3, §14.5)

- **Zéro agent** : pas de bouton `^E`, pas de zone cible.
- **Un agent** : bouton `^E`, **pas** de zone cible — elle n'apprendrait rien.
- **Deux agents ou plus** : bouton `^E` et zone `→ claude·p3`, le suffixe étant
  la partie du `pane_id` après le `:`. C'est le seul discriminant à la fois
  unique, stable et vérifiable au `herdr pane list` ; il résout le point ouvert
  n°1 du `CLAUDE.md`.

`Ctrl+N` et le clic sur la zone ne font **rien**, sans message, en dessous de
deux agents — cohérent avec §14.5, où le cyclage n'a jamais de retour.

### 3.9 Le canal bidirectionnel : une ligne de README

Le README publie déjà le chemin du buffer en dur (`README.md:96` et `:99`). Il
gagne un suffixe, et c'est tout le travail :

```
cat ~/.local/state/herdr/plugins/herdr-scratchpad/scratchpad-$HERDR_TAB_ID.txt
```

`HERDR_TAB_ID` est présent dans le pane de l'agent comme dans les autres. **Ne
pas** ajouter d'action au manifeste, ni de mode `--state-path`, ni de séquence
`plugin action invoke` + `plugin log list` : elles ont été conçues, puis
écartées, parce qu'elles remplacent une ligne de `cat` par deux commandes et un
aller-retour pour publier une information constante.

### 3.10 L'export est suffixé lui aussi (§10)

`/tmp/herdr-scratchpad-<tab_id>.txt`. Tant que le buffer était global, deux
exports écrivaient le même texte ; maintenant, un chemin fixe ferait écraser
silencieusement l'instantané d'une autre tab — au seul endroit que personne ne
surveille. Le chemin reste une adresse : l'agent a `HERDR_TAB_ID` pour la
composer.

### 3.11 Nouvel ordre de la barre (§14.3)

```
0 agent    ^C copier · ^L vider · ^S fichier
1 agent    ^C copier · ^L vider · ^S fichier · ^E envoyer
2 agents   ^C copier · ^L vider · ^S fichier · ^E envoyer · → claude·p3
```

Les commandes **fixes** passent à gauche, les éléments **variables** à droite.
Raison : la liste d'agents est rafraîchie toutes les 2,5 s, donc `^E` et la
zone cible apparaissent et disparaissent tout seuls. Dans l'ordre actuel, un
agent qui démarre décale `^C`, `^L` et `^S` pendant que le doigt descend vers
eux. Dans celui-ci, ils ne bougent **jamais** — ni selon les agents, ni selon
la largeur.

Le rognage **ne change pas** : il continue de partir de la droite, donc `^E` et
la cible tombent en premier sur une barre étroite. C'est le corollaire assumé
de l'inversion, et l'argument de §14.3 (« ne jamais pouvoir envoyer sans lire
où ») a fondu : la cible est maintenant locale à la tab, et `Ctrl+E` reste au
clavier.

### 3.12 Ne changent pas

`Ctrl+C`, `Ctrl+L`, `Ctrl+Z`, `Ctrl+S`, l'édition, le repliage, la souris, le
heartbeat, le lanceur, le manifeste, `prefix+a`, le dépôt `keys: []`, la
revérification de la cible à l'envoi, la bascule du focus après un dépôt
réussi, et le fait que le scratchpad reste ouvert après l'envoi.

Un mot sur ce dernier point : maintenant que l'agent est le pane d'à côté,
`agent.focus` ne change plus de tab et le scratchpad **reste visible**. Le coût
annoncé en §14.9 (« le `^Z` du dépôt n'est plus à une touche ») s'allège de
lui-même. Ne pas en profiter pour fermer le pane après l'envoi : la case de
secours (`stash`) vit en mémoire, la fermer la détruit.

## 4. Faits vérifiés (ne pas re-découvrir)

Vérifiés dans les sources de herdr, pas devinés.

### Les identifiants de tab sont sûrs comme clé

- Forme : `w<n>:t<n>` (`herdr src/workspace.rs:150`), alphanumérique plus `:`.
  Nom de fichier légal sous unix ; Windows est hors périmètre.
- **Stables et jamais réutilisés après fermeture** :
  `tab_public_numbers_are_stable_and_not_reused_after_close`
  (`herdr src/workspace.rs:1593`).
- **Survivent au redémarrage** : le snapshot capture `public_tab_numbers` et
  `next_public_tab_number` (`herdr src/persist/snapshot.rs:979`), et la
  restauration préserve le mapping (`herdr src/persist/restore.rs:1251`).

Conséquence : aucune tab neuve n'héritera jamais du buffer d'une tab défunte,
et le ménage de §3.6 ne peut pas supprimer un buffer qui redeviendrait vivant.

### `tab.list` répond en un seul appel

`TabListParams.workspace_id` est **optionnel**
(`herdr src/api/schema/tabs.rs:21`) ; sans lui, le handler parcourt tous les
workspaces (`herdr src/app/api/tabs.rs:21-30`). Réponse :

```json
{"result":{"tabs":[{"tab_id":"w2:t1","workspace_id":"w2","number":1,
                    "label":"…","focused":false,"pane_count":2,
                    "agent_status":"idle"}]}}
```

### `agent.list` porte déjà le `tab_id`

```json
{"agent":"claude","pane_id":"w2:p1","tab_id":"w2:t1","workspace_id":"w2"}
```

Le filtre de §3.7 est donc une comparaison de chaînes, sans appel
supplémentaire. `workspace.list` devient inutile : un appel socket se libère à
chaque rafraîchissement, ce qui paye celui de `tab.list` au démarrage.

### État réel sur la machine de référence

```
~/.local/state/herdr/plugins/herdr-scratchpad/scratchpad.txt   (0 octet)
~/.local/state/herdr/plugins/herdr-scratchpad/target.txt
~/.config/herdr/scratchpad/target.txt
```

Les trois sont à supprimer (§3.5). Vérifier que `scratchpad.txt` est toujours
vide au moment de livrer ; s'il ne l'est plus, prévenir avant de supprimer.

## 5. Travail, fichier par fichier

Ordre conseillé : `state.rs` d'abord (la clé), puis `agents.rs` (la cible),
puis `ipc.rs`, `ui.rs`, `app.rs`, puis la documentation.

### 5.1 `src/state.rs` — la clé, le ménage, la purge

```rust
/// Emplacement du buffer de `tab_id`, ou du buffer sans clé hors herdr.
pub fn from_env(tab_id: Option<&str>) -> Option<Self>;

/// Supprime l'ancien buffer global et les `target.txt` de `dir`.
/// Idempotent, muet.
pub fn purge_legacy(dir: &Path);

/// Supprime dans `dir` les buffers dont la tab n'existe plus.
///
/// `live_tab_ids` vient de `tab.list`. `own` n'est jamais supprimé.
pub fn sweep_orphans(dir: &Path, live_tab_ids: &[String], own: &Path);

/// Les deux répertoires à balayer : le state dir de herdr et le repli.
pub fn state_dirs() -> Vec<PathBuf>;
```

Les deux prennent un **répertoire en argument** : c'est ce qui permet de les
tester dans un répertoire jetable nommé d'après le pid, comme l'exige
`CLAUDE.md`. C'est l'appelant qui les invoque pour chaque entrée de
`state_dirs()`.

- nom du fichier : `format!("scratchpad-{tab_id}.txt")`, ou `scratchpad.txt`
  quand `tab_id` est `None` ;
- **supprimer** `TARGET_FILE`, `load_target`, `save_target`, `read_target` et
  leurs tests ;
- `sweep_orphans` ne regarde que les noms au motif `scratchpad-*.txt` — le
  fichier sans suffixe n'est donc jamais candidat ;
- la décision d'abstention (liste vide ou en erreur) appartient à l'**appelant**
  dans `app.rs`, qui a le JSON : `sweep_orphans` reçoit une liste déjà validée
  et non vide. Garder la fonction bête la rend testable sans socket ;
- ne pas toucher à l'écriture atomique. Le `.tmp.<pid>` devient une ceinture
  sans bretelles (un seul scratchpad par tab, donc plus jamais deux écrivains
  sur un fichier) — le garder ne coûte rien et couvre le repli sans clé.

### 5.2 `src/agents.rs` — rétrécir

```rust
pub struct Target {
    pub pane_id: String,
    pub agent: String,
}

/// Les agents de `tab_id`, ordonnés par `pane_id`.
pub fn targets(agents_json: &str, tab_id: Option<&str>, exclude_pane: Option<&str>) -> Vec<Target>;

/// Cible suivante, en boucle. Inchangée.
pub fn next(targets: &[Target], current: Option<usize>) -> Option<usize>;

/// `→ claude·p3`, rogné à `width` colonnes. Le nom de l'agent survit au suffixe.
pub fn label(target: &Target, width: usize) -> String;
```

- `tab_id` à `None` (hors herdr) rend une liste **vide** : pas de tab, pas de
  cible. C'est cohérent avec §3.7 et évite qu'un binaire lancé à la main puisse
  déposer du texte quelque part ;
- **supprimer** `pick_default`, `Home`, `NO_TARGET`, `workspace_label`,
  `workspace_id`, `tab_id` du `Target` (il est constant par construction), et
  tout le croisement avec `workspace.list` ;
- `label` ne prend plus d'`Option` : l'absence de cible ne s'affiche plus, elle
  se traduit par l'absence de la zone (§3.8). C'est `ui.rs` qui décide de
  l'afficher, à partir de `targets.len() >= 2` ;
- suffixe : la partie du `pane_id` après le dernier `:`, ou le `pane_id` entier
  s'il n'en contient pas.

### 5.3 `src/ipc.rs` — une méthode de plus, une de moins

- ajouter `fn tab_list(&self) -> Option<String>` au trait `Herdr` et à `Socket`
  (`tab.list`, params `{}`) ;
- **supprimer** `workspace_list` du trait, de `Socket` et du faux des tests ;
- rien d'autre ne bouge. Règle du projet : jamais d'appel direct à `ipc::`
  depuis `app.rs`, tout passe par le trait.

### 5.4 `src/ui.rs` — inverser l'ordre

`bar_labels` prend le nombre de cibles en plus de la cible :

```rust
fn bar_labels(target: Option<&Target>, target_count: usize, bar_width: usize)
    -> Vec<(Action, String)>
```

Construction, dans l'ordre :

1. `^C copier`, `^L vider`, `^S fichier` — toujours ;
2. `^E envoyer` — si `target_count >= 1` ;
3. la zone cible — si `target_count >= 2`.

Ne pas toucher au rognage ni à `button_rects` : l'ordre reste la priorité. Le
budget de largeur de la zone cible (`bar.width / 3`, plancher 13) peut être
revu à la baisse, le libellé étant beaucoup plus court qu'avant.

Les tests d'ordre et de rognage sont à **mettre à jour**, pas à supprimer, et
il en faut trois nouveaux (§6).

### 5.5 `src/app.rs` — recâbler

- `App::new` : lire `HERDR_TAB_ID` **avant** le `Store`, et le passer à
  `Store::from_env`. Puis, dans cet ordre : `purge_legacy()`, le ménage
  (§5.6), le chargement du texte, `stamp()`, `refresh_targets()` ;
- champ `home: Home` → `tab_id: Option<String>` ; supprimer `remembered` ;
- `refresh_targets` : un seul appel (`agent_list`), filtre par `tab_id`,
  exclusion du pane courant conservée. Préserver la cible sélectionnée par
  `pane_id` à travers un rafraîchissement ; sinon retomber sur l'index 0 ;
- `CycleTarget` : ne rien faire si `targets.len() < 2`, sans message ;
- `send()` : inchangé, sauf la disparition de `save_target`. La revérification
  au moment de l'envoi reste la re-résolution complète, filtre de tab compris ;
- `export()` : suffixer le chemin avec le `tab_id` quand il existe.

### 5.6 Le ménage, dans `app.rs`

Au démarrage seulement, jamais dans le rafraîchissement :

```
let Some(json) = herdr.tab_list() else { return };   // pas de serveur : abstention
let ids = agents::live_tab_ids(&json);               // pur, dans agents.rs
if ids.is_empty() { return }                         // erreur ou panne : abstention
for dir in state::state_dirs() {
    state::sweep_orphans(&dir, &ids, own_path);
}
```

`live_tab_ids` est une fonction **pure** de plus dans `agents.rs` : elle rend
la liste des `tab_id` d'un JSON de `tab.list`, et une liste vide sur du JSON
illisible — ce qui déclenche l'abstention par construction.

## 6. Documentation à mettre à jour

`DESIGN.md`, **réécriture in situ** avec une note de révision datée du
2026-08-25, dans la forme de celle de §14.4 :

| Section | Ce qui change |
| --- | --- |
| §8 | nom du fichier, purge de l'ancien, ménage |
| §9 | **renversée** : ce n'est plus « un texte partout » mais « un texte par tab » ; le transit inter-workspace passe par `Ctrl+S` et `Ctrl+C`, et c'est écrit |
| §10 | chemin d'export suffixé |
| §14.3 | libellé sans workspace, zone affichée à partir de deux agents, nouvel ordre de barre |
| §14.4 | portée réduite à la tab, replis supprimés |
| §14.5 | cyclage inerte en dessous de deux agents |
| §14.7 | **supprimée** : il n'y a plus de mémoire de cible |
| §14.9 | le coût annoncé s'allège, l'agent étant dans la même tab |

`README.md` : chemin du buffer suffixé (`:91`, `:96`, `:99`), chemin d'export
(`:21`, `:125`), et un mot sur le cloisonnement par tab.

`CLAUDE.md` : la ligne « cloisonnement par workspace — le buffer est global,
c'est le point » est renversée ; le point ouvert n°1 (deux agents
indiscernables) **tombe**, résolu par le suffixe de `pane_id` ; le point ouvert
n°4 (scratchpad seul dans sa tab) devient un comportement normal et documenté ;
ajouter aux pièges le fait que `HERDR_TAB_ID` est figé au spawn (§3.3).

## 7. Tests à écrire

Style existant : noms en français, une assertion par comportement, le message
d'assertion dit *pourquoi*. Aucun socket nulle part ; pas de disque sauf dans
`state.rs`, dans un répertoire jetable nommé d'après le pid.

**`agents.rs`**

- un agent d'une autre tab n'est pas une cible ;
- deux agents de la même tab sont deux cibles, ordonnées par `pane_id` ;
- une tab inconnue ne rend aucune cible ;
- `tab_id` absent ne rend aucune cible ;
- le pane courant reste exclu même s'il est dans la bonne tab ;
- `label` rend `→ claude·p3` ;
- `label` rogne le suffixe avant le nom de l'agent ;
- `live_tab_ids` rend les ids d'un `tab.list` bien formé ;
- `live_tab_ids` rend une liste vide sur du JSON illisible ;
- du JSON illisible ne panique nulle part.

**`state.rs`** (répertoire jetable)

- le nom du fichier porte le `tab_id` ;
- sans `tab_id`, le nom est `scratchpad.txt` ;
- `purge_legacy` supprime l'ancien global et le `target.txt` du répertoire, et
  ne touche pas aux fichiers suffixés ;
- `purge_legacy` sur un répertoire vide ne panique pas ;
- `sweep_orphans` supprime un buffer dont la tab manque de la liste ;
- `sweep_orphans` **conserve** le fichier passé en `own`, même absent de la
  liste ;
- `sweep_orphans` ne touche pas au `scratchpad.txt` sans suffixe.

**`app.rs`** (avec `FakeHerdr`)

- `Ctrl+N` ne fait rien et ne dit rien quand il n'y a qu'un agent ;
- `Ctrl+N` cycle quand il y en a deux, sans toucher au texte ;
- un envoi vers un agent d'une autre tab est impossible : la liste est vide,
  donc le bouton n'existe pas ;
- le ménage ne s'exécute pas quand `tab.list` rend une liste vide (vérifier via
  le faux que `sweep` n'a pas été appelé, ou qu'aucun fichier n'a disparu) ;
- inchangés à ne pas casser : un envoi qui échoue ne vide pas, un envoi réussi
  vide et `Ctrl+Z` rattrape.

**`ui.rs`**

- sans agent, ni `^E` ni zone cible ne sont dans la barre ;
- avec un agent, `^E` est là mais pas la zone ;
- avec deux agents, les deux sont là ;
- **`^C`, `^L` et `^S` occupent les mêmes rectangles dans les trois cas** —
  c'est le test qui protège la raison d'être de l'inversion.

## 8. Recette manuelle

Rappel de `CLAUDE.md` : **fermer et rouvrir le pane après chaque
`cargo build --release`**, et vérifier à quel pane on parle avant d'envoyer
quoi que ce soit (`pgrep -f 'target/release/herdr-scratchpad$'`, puis
`HERDR_PANE_ID` dans `/proc/<pid>/environ`).

```bash
H=~/.local/bin/herdr
D=~/.local/state/herdr/plugins/herdr-scratchpad

# 1. Purge : après le premier démarrage, ces trois-là ont disparu
ls "$D"; ls ~/.config/herdr/scratchpad/

# 2. Cloisonnement : deux tabs, deux textes
#    ouvrir un scratchpad dans la tab A, y coller "aaa"
#    ouvrir un scratchpad dans la tab B, y coller "bbb"
ls "$D"                      # deux fichiers suffixés, contenus distincts
$H pane read <pane A> | tail -c 120
$H pane read <pane B> | tail -c 120

# 3. Canal bidirectionnel, depuis le pane de l'agent
echo test > "$D/scratchpad-$HERDR_TAB_ID.txt"   # le scratchpad se recharge

# 4. Barre : trois formes
#    tab sans agent      -> ni ^E ni zone
#    tab à un agent      -> ^E, pas de zone
#    tab à deux agents   -> ^E et "→ claude·pN"
#    et ^C/^L/^S au même endroit dans les trois cas

# 5. Envoi : le texte arrive chez l'agent de MA tab, non soumis
$H pane send-keys <pane scratchpad> ctrl+e

# 6. Export suffixé
$H pane send-keys <pane scratchpad> ctrl+s
ls /tmp/herdr-scratchpad-*.txt

# 7. Ménage : fermer la tab B, rouvrir un scratchpad ailleurs
ls "$D"                      # le buffer de B a disparu, celui de A est intact
```

Nettoyage après recette : `pane send-keys <agent> ctrl+u` défait un collage
déposé dans la boîte de saisie d'un agent.

## 9. Ce qu'il ne faut PAS faire

- **résoudre la tab dynamiquement** (`pane.get` sur soi-même) — écarté en §3.3 ;
- **ajouter une action au manifeste** ou un mode `--state-path` — écartés en
  §3.9, le README suffit ;
- **migrer l'ancien buffer global** vers une tab — §3.5 ;
- **supprimer un buffer parce que `tab.list` a répondu vide** — c'est une
  panne, pas une information (§3.6) ;
- **rouvrir la cible au workspace** quand la tab n'a pas d'agent — §3.7 ;
- **afficher la zone cible avec un seul agent** — §3.8 ;
- **inverser la priorité de rognage** — §3.11 ;
- **fermer le scratchpad après un envoi réussi** — la case de secours vit en
  mémoire (§3.12) ;
- **toucher au format du fichier** (texte nu), au dépôt `keys: []`, au
  heartbeat, au lanceur ou au manifeste ;
- **réintroduire un découpage ligne à ligne** de l'envoi — le bracketed paste
  s'en charge, c'est écrit dans `CLAUDE.md`.
