# herdr-scratchpad — conception

> Document issu d'une session de questionnement structuré du 2026-08-24.
> Toutes les décisions ci-dessous ont été posées une par une et validées.
> Aucune ligne de code n'a encore été écrite : ce document précède l'implémentation.

## 1. Ce que c'est

Un **presse-papier éditable persistant**, dans un pane herdr.

Un outil de **transit** : on y colle, on y récupère, on vide. Ce n'est pas un
carnet — on ne relit pas un scratchpad, on s'en sert et on le jette.

Cette distinction est la racine de tout le reste. Elle justifie l'existence du
plugin à côté de [`herdr-notes`](https://github.com/alexarthurs/herdr-notes),
qui couvre déjà le panneau, la saisie, l'autosave et le clear — mais en tant que
**carnet** : markdown rendu, mode preview/édition, une note par workspace.

Conséquences directes, non négociables sans rouvrir la racine :

- pas de markdown rendu, pas de mode preview ;
- une zone de texte brute **toujours éditable**, sans aucun mode ;
- **un seul buffer global** — pas de cloisonnement par workspace. Le
  cloisonnement est une vertu de carnet ; ici on colle depuis le projet A
  précisément pour récupérer dans le projet B.

## 2. Contexte matériel

La machine de développement et d'usage est un **Raspberry Pi headless, atteint
en SSH**. Pas de X, pas de Wayland, ni `xclip`, ni `xsel`, ni `wl-copy`, ni
`pbcopy`. Écran client réduit.

Ce n'est pas une anecdote : ça détermine le mécanisme de copie (§5) et la
portée du projet (§11).

## 3. Le panneau

- Docké **en bas**, ~30 % de hauteur, pleine largeur — via le **script
  lanceur**, obligatoirement. Le manifeste ne sait pas l'exprimer : `placement`
  n'accepte que `overlay|popup|split|tab|zoomed`, il n'a ni `direction` ni
  `ratio`, et un split déclaratif est câblé en 50/50 vers la droite
  (`ratio.unwrap_or(0.5)`, herdr `src/workspace/tab.rs:361`). L'entrée
  `[[panes]]` du manifeste n'est qu'un repli dégradé mais fonctionnel.
  Le texte d'un scratchpad est *large* — chemins, commandes, URLs, blocs collés.
  La largeur est la ressource utile, pas la hauteur. C'est aussi le seul
  placement où la barre de boutons est confortable au doigt sur petit écran.
  (`herdr-notes` est à droite parce qu'une note se lit en colonne.)
- **Toggle scopé à la tab** : ouvre docké en bas / focalise si ouvert / ferme si
  focalisé. Mécanique de *heartbeat* reprise telle quelle de `herdr-notes` pour
  qu'un pane mort — y compris laissé par un redémarrage du serveur herdr — soit
  remplacé et jamais dupliqué.
- Raccourci : `prefix+a` (libre ; `prefix+f`/`prefix+shift+f` sont au
  file-viewer, `prefix+t` à Notes).
- `prefix+shift+a` : variante tab dédiée plein écran, pour les moments où on
  veut vraiment écrire.
- Retour à la ligne automatique sur les lignes longues.

## 4. Commandes et touches

`Ctrl+B` est le préfixe de herdr, c'est la **seule** combinaison confisquée
globalement. Le mode raw neutralise `Ctrl+C`, `Ctrl+Z` et `Ctrl+S` avant que le
terminal n'en fasse des signaux : ils arrivent au pane comme des touches.

| Touche | Action |
| --- | --- |
| `Ctrl+E` | envoyer à un agent (§14) |
| `Ctrl+N` | agent suivant (§14) |
| `Ctrl+C` | copier tout |
| `Ctrl+L` | vider |
| `Ctrl+S` | exporter dans un fichier |
| `Ctrl+Z` | rattraper le dernier vidage |

Les quatre dernières sont les combinaisons que tout le monde connaît déjà,
chacune à sa signification habituelle : copier, effacer l'écran, sauvegarder,
annuler. Rien à mémoriser. Seule bizarrerie assumée : `Ctrl+C` n'interrompt plus
— mais dans un scratchpad il n'y a rien à interrompre.

`Ctrl+E` et `Ctrl+N` sont arrivés plus tard (§14) et n'ont pas cette évidence :
`E` pour *envoyer*, `N` pour *next*. Elles étaient libres précisément parce que
§4 refuse le readline.

**Aucune touche pour quitter.** `prefix+a` referme le pane, geste symétrique de
celui qui l'a ouvert. Un `Ctrl+Q` sur un panneau qu'on veut permanent ne sert
qu'à le fermer par erreur.

### Édition : minimale

Flèches, `Backspace`, `Suppr`, `Home`/`End`, `PgUp`/`PgDn`, `Entrée`. Rien
d'autre.

**Pas de readline/emacs** (`Ctrl+A`, `Ctrl+E`, `Ctrl+K`, `Ctrl+W`, `Ctrl+U`) :
ces lettres sont plus utiles aux commandes quotidiennes qu'à un confort
d'édition fine qu'on utilise une fois par mois. Le pari s'est vérifié : `Ctrl+E`
est devenu « envoyer » (§14). Quand un texte mérite vraiment
d'être édité, sa place est dans un éditeur, pas dans le pane de transit.

Corollaire : **pas d'undo de frappe**. `Ctrl+Z` est entièrement réservé au
rattrapage du vidage (§7) — et à celui de l'envoi, qui emprunte le même chemin
(§14.2).

### Souris

Capture **intégrale**.

> **Correction du 2026-08-24, à l'implémentation.** Le compromis annoncé lors de
> la session — « la sélection native est perdue » — était trop pessimiste.
> herdr **réserve `Shift`+souris à la sélection du terminal**, et les plugins
> doivent laisser passer ces événements sans y toucher (règle appliquée partout
> dans `herdr-file-viewer` : `if ev.modifiers.contains(SHIFT) { return noop }`).
> `Shift`+glisser continue donc de sélectionner normalement dans le pane. La
> capture ne coûte que la sélection *sans* modificateur.

Le `copy_mode` clavier de herdr (`prefix`+…) reste disponible en plus.

Réimplémenter le glisser-sélectionner à l'intérieur (comme le fait
`herdr-file-viewer` dans `src/preview.rs`) a été écarté : c'est de loin la
partie la plus chère du projet, pour un outil dont la copie est par définition
« **tout** copier » — et la correction ci-dessus lui retire son dernier
argument.

Barre du bas cliquable, une seule ligne :

```
[^E envoyer] [→ claude·wdv] [^C copier] [^L vider] [^S fichier] [^Z annuler]
```

Le destructif n'est pas au bord, là où le pouce dérape.

## 5. Copier

**OSC 52**, vers le presse-papier du terminal hôte — c'est-à-dire de la machine
où l'on va coller, pas du Pi. Sur une machine headless en SSH, c'est le seul
mécanisme possible, et il est meilleur que `xclip` ne l'aurait été.

Vérifié dans les sources de herdr :

- `src/pane/osc.rs` parse l'OSC 52 émis par un enfant de pane ;
- `src/events.rs:136` — « A pane child emitted a valid OSC 52 clipboard write » ;
- `src/protocol/wire.rs:701` relaie la donnée à travers le serveur ;
- `src/selection.rs` détecte les sessions SSH pour **préférer** OSC 52 aux
  outils natifs.

Aucun réglage à activer : c'est le cas nominal.

**Plafond dur : 192 Ko** (`MAX_CLIPBOARD_BYTES`, `src/ghostty/mod.rs:468`).
Au-delà, herdr jette la copie. Le pane doit donc **refuser explicitement** la
copie au-delà de ce seuil et renvoyer vers l'export (§8) dans la barre du bas,
plutôt que de laisser un collage silencieusement vide — le pire échec possible
pour un outil de transit.

## 6. Retour visuel

Une **seule ligne**, en bas, qui fait les deux : les boutons y vivent, et le
retour s'y affiche à leur place ~3 secondes (`copié · 1,2 Ko`, ou le chemin
d'export) avant qu'ils ne reviennent. Coût fixe : une ligne, jamais plus.

Masquer les boutons pendant ce temps est sans conséquence : on ne reclique pas
dans la seconde.

`herdr notification show` (toast au niveau de herdr) a été écarté : ça ferait
dépendre un geste local d'un aller-retour socket pour économiser zéro ligne, et
mélangerait les retours du scratchpad aux notifications des agents.

## 7. Vider

Immédiat, **sans confirmation**. L'ancien contenu part dans une case de secours
à **une seule place** ; `Ctrl+Z` le ramène. L'envoi (§14) réutilise exactement
ce chemin.

Vider est la moitié du métier d'un outil de transit : ça doit coûter une frappe.
Une confirmation y remettrait exactement le mode modal chassé partout ailleurs.
Le regret d'un vidage se manifeste dans les trois secondes, pas trois vidages
plus tard — une case suffit.

## 8. Persistance

**`scratchpad.txt`, texte brut**, dans `HERDR_PLUGIN_STATE_DIR`.

Le fichier d'état *est* le texte. Pas de JSON : sans mode ni métadonnée à ranger
à côté, il ne servirait qu'à échapper le contenu et à le rendre illisible au
`cat` et inutilisable au `grep`.

C'est la meilleure affaire du design : le scratchpad devient un **canal
bidirectionnel** presque gratuit. On écrit dans le pane, un agent lit le
fichier ; un agent écrit dans le fichier, le pane se recharge tout seul (§9).

- Écriture **atomique** : fichier temporaire + `fsync` + `rename`.
- Autosave **debouncée ~500 ms** après la dernière frappe — plus court que les
  2 s de `herdr-notes`, parce qu'ici le fichier sert de canal vers les agents.
- Sauvegarde aussi au vidage et à la fermeture.
- Fichier absent ou illisible → buffer vide. Ça ne doit jamais bloquer le pane.
- Hors herdr (`HERDR_PLUGIN_STATE_DIR` absent) : repli sur le répertoire de
  config de la plateforme.

## 9. Plusieurs panes ouverts

Autorisés. Le **fichier est la source unique de vérité** : chaque pane surveille
son mtime et se recharge quand il change *et* qu'il n'a pas lui-même de frappes
non sauvegardées.

C'est la seule option qui tient la promesse du buffer global : *un* texte,
**partout**. Un singleton la trahirait à moitié — un texte unique mais un seul
endroit d'où y accéder, ce qui rate le cas « je colle depuis A, je récupère dans
B ».

Le risque de collision est théorique : il n'y a qu'un humain, il ne tape que
dans un pane à la fois, et la fenêtre réelle est celle de l'autosave.

## 10. Export

`/tmp/herdr-scratchpad.txt` — chemin **fixe**, **écrasé** à chaque export, et
affiché ~3 s dans la barre du bas.

Son rôle a changé une fois le fichier d'état passé en texte brut (§8) : ce n'est
plus « sortir le texte », c'est **figer un instantané**. Un fichier d'état qui
bouge tout seul n'est pas un instantané ; quand on veut geler l'état pour s'en
servir ailleurs, il faut une copie qui ne changera plus sous les pieds.

C'est aussi le recours au-delà des 192 Ko d'OSC 52 (§5).

Un chemin stable est une adresse : les scripts et les agents peuvent le lire
sans qu'on leur dise où. `/tmp` est vidé au reboot, sans importance — la vraie
persistance est ailleurs (§8).

## 11. Collage

**Bracketed paste activé** — obligatoire, pas optionnel : c'est l'entrée
principale de l'outil. Sans lui, un collage de 50 Ko arrive touche par touche.

herdr le fournit dès que le pane active `?2004h` : il enveloppe alors le texte
collé dans `\x1b[200~…\x1b[201~` (`src/pane.rs:2858`). Le collage arrive comme
**un seul événement**.

Insertion **au curseur**, comportement d'éditeur standard. L'accumulation en
pile (« toujours ajouter à la fin ») a été écartée : elle retire le contrôle
dans tous les autres cas, et le comportement standard la fait déjà quand on la
veut (`Ctrl+End` puis coller).

## 12. Portée du projet

**Outil personnel maintenant, structuré pour être publiable plus tard.**

- Rust + ratatui. Non discuté : les deux implémentations de référence dont on va
  reprendre des morceaux entiers (heartbeat du toggle de `herdr-notes`,
  hit-testing souris de `herdr-file-viewer`) sont en Rust, et `[[build]]` du
  manifeste attend un `cargo build --release`.
- `platforms = ["linux", "macos"]`. **Pas de Windows** : c'est la moitié du coût
  du plugin (le `CLAUDE.md` de `herdr-notes` fait 13 Ko presque entièrement
  consacré à ses pièges — spawn de pane en chemin relatif impossible, lanceur
  PowerShell, ids d'action suffixés, AltGr qui arrive en `CTRL|ALT`) et il est
  intestable depuis un Pi headless. Écrire du code non vérifiable pour un
  utilisateur hypothétique est le mauvais échange.
- Pas de CI, pas de LICENSE, README court, `herdr plugin link .`.
- Manifeste et arborescence calqués sur `herdr-notes` dès le départ, pour que
  la publication reste une soirée de travail et pas une réécriture.

## 13. Détails tranchés par défaut

Sans branche de conception à ouvrir dessus :

1. Retour à la ligne automatique sur les lignes longues.
2. Molette de souris = défilement (la souris est capturée de toute façon).
3. Texte d'aide quand le buffer est vide, qui disparaît à la première frappe.
4. Autosave ~500 ms (§8).
5. Repli sur le répertoire de config de la plateforme hors herdr (§8).

## 14. Envoyer à l'agent

> Ajout du 2026-08-24, même méthode : décisions posées une par une avant la
> première ligne de code.

Un bouton `[^E envoyer]` et une zone `[→ claude·wdv]` en tête de la barre du
bas. Le texte du scratchpad est **déposé** dans la boîte de saisie d'un agent
herdr, puis le scratchpad se vide dans sa case de secours.

C'est la suite naturelle du canal bidirectionnel (§8) : le fichier d'état donne
déjà « un agent lit ce que j'ai collé », il manquait « cet agent-là, maintenant,
sans que j'aille moi-même chercher son pane ».

### 14.1 Déposer, pas soumettre

`pane.send_input` avec `keys: []`. Le texte atterrit dans le champ de saisie ;
**aucune Entrée n'est envoyée**. L'utilisateur bascule, relit, soumet lui-même.

`agent.prompt` existe et soumettrait directement. Il est **écarté délibérément** :
déposer rend l'envoi réversible côté agent, et c'est le seul garde-fou qui
survit à une erreur de cible.

Effet de bord bienvenu : déposer chez un agent **occupé** est inoffensif, le
texte attend dans la boîte. Il n'y a donc aucun cas « agent en train de
travailler » à traiter — ce qui supprime tout un pan de logique.

Le multiligne est sûr sans découpage : herdr enveloppe le texte dans un
*bracketed paste* dès que le pane a activé `?2004h` (`src/pane.rs:2858`), ce que
fait Claude Code. Un prompt de dix lignes arrive comme **un seul collage** et ne
se fait pas soumettre à la première ligne. C'était le risque principal de la
fonctionnalité ; il n'existe pas.

### 14.2 Le scratchpad se vide après un dépôt réussi

Même chemin que `Ctrl+L` : le texte part dans la case de secours à une place,
`Ctrl+Z` le rattrape. Le dépôt est un *déplacement*, pas une copie — un outil de
transit dont le contenu survit à son usage se transforme en carnet par
accumulation, ce que §1 refuse.

**En cas d'échec, on ne vide pas.** C'est ce qui rend l'erreur sans conséquence,
et donc ce qui autorise l'absence de confirmation.

### 14.3 La cible est affichée en permanence

C'est le garde-fou principal, et il remplace toute confirmation modale — le
plugin n'a aucun mode (§1) et n'en gagne pas ici.

« Envoyer » est la seule commande **sortante** : les quatre autres restent chez
l'utilisateur, celle-ci démarre du travail ailleurs. Un bouton dont on ne peut
pas lire la destination avant d'appuyer est un piège, surtout avec plusieurs
agents `claude` que rien ne distingue.

Libellé `→ <agent>·<label du workspace>`. Quand la barre est serrée, le
workspace tombe **entier** avant que le nom de l'agent ne soit entamé : un
`→ claude·herdr-scr` tronqué désignerait aussi bien un autre workspace
commençant pareil.

### 14.4 Choix de la cible par défaut

1. l'agent de la **même tab que ce pane** (`HERDR_TAB_ID`) ;
2. sinon l'agent du **même workspace** (`HERDR_WORKSPACE_ID`) ;
3. sinon la **dernière cible utilisée**, mémorisée sur disque ;
4. sinon le **premier agent disponible** ;
5. sinon aucune : `→ aucun agent`, et le bouton refuse au clic.

> **Correction du 2026-08-24, au test en vrai.** La règle s'arrêtait d'abord au
> workspace. C'était insuffisant : un workspace porte plusieurs tabs, donc
> plusieurs agents, et c'est le *voisin* qui sortait — pas celui qu'on
> regardait. « L'agent du pane courant » se lit dans la **tab**, parce que le
> scratchpad naît d'un split du pane focalisé et hérite donc de sa tab.

Le repli sur le workspace reste utile : un scratchpad **seul dans sa tab** n'a
aucun agent chez lui (§3 prévoit déjà ce cas pour la fermeture).

### 14.5 Cyclage

Clic sur la zone cible, **ou** `Ctrl+N`. Les deux voies existent parce que la
barre existe pour être utilisable au doigt sur petit écran : une commande
dangereuse ne doit pas être la seule à exiger le clavier.

`Tab` a été écarté : dans une zone de texte il doit insérer une tabulation, et
un scratchpad qui mange les tabulations d'un bloc de code collé trahit sa
fonction.

### 14.6 Fraîcheur de la cible

Rafraîchissement de l'affichage toutes les **2,5 s**, et **revérification au
moment de l'envoi**. Si la cible affichée a disparu → message, rien n'est envoyé,
rien n'est vidé — et surtout, on ne se rabat **pas** sur une autre cible : ce
serait envoyer ailleurs que ce que l'utilisateur a lu.

L'affichage n'a besoin d'être qu'à peu près à jour ; l'action doit être
exactement juste.

### 14.7 La mémoire de la cible n'est pas dans `scratchpad.txt`

Ce fichier reste du **texte nu** : c'est tout le contrat du canal
bidirectionnel (§8). La dernière cible va dans un fichier voisin `target.txt`.

Contenu : `<label du workspace>\t<agent>` — **pas** le `pane_id`, qui ne survit
pas à un redémarrage de herdr. La résolution au démarrage re-cherche un agent
correspondant à cette paire.

### 14.9 Le focus suit le texte

Un dépôt réussi **bascule sur l'agent** qui vient de recevoir (`agent.focus`,
qui change tab *et* workspace au besoin — `herdr src/app/agents.rs:75`).

C'est la fin du geste annoncé en §14.1 : déposer, relire, soumettre. Sans la
bascule, la troisième étape commence par « retrouver le pane soi-même », c'est-à-
dire exactement le travail que le bouton devait épargner.

Le focus va au pane qui a **effectivement** reçu, pas à celui que l'affichage
montrait : c'est la cible re-résolue de §14.6.

Un échec de bascule est avalé — le texte est déjà parti, et rater le focus ne
doit pas transformer un envoi réussi en message d'erreur. Un envoi qui échoue,
lui, ne bascule pas du tout : le texte est encore sous les yeux.

Ce que ça coûte, assumé : le `^Z` du dépôt n'est plus à une touche, il faut
revenir par `prefix+a`. La case de secours, elle, survit intacte. Les variantes
« basculer seulement dans la même tab » et « deux touches, une par sens » ont
été écartées : la première rend le comportement conditionnel donc imprévisible
(le plugin n'a aucun mode, §1), la seconde ajoute une touche et un bouton pour
un choix qu'on fait toujours dans le même sens.

### 14.8 Pas de garde-fou de taille

`MAX_INPUT_PAYLOAD` vaut 1 Mo (`herdr src/server/client_transport.rs:41`), cinq
fois le plafond presse-papier de §5. Inatteignable pour un prompt : ajouter un
seuil ici serait du code qui ne s'exécute jamais.

## Annexe — code de référence local

| Chemin | Ce qu'on y prend |
| --- | --- |
| `~/workspace/github.com/herdrdev/herdr` | sources herdr : OSC 52, bracketed paste, plafonds |
| `~/workspace/github.com/alexarthurs/herdr-notes` | heartbeat du toggle, manifeste, écriture atomique |
| `~/.config/herdr/plugins/github/herdr-file-viewer-*` | hit-testing souris (`src/presenter.rs`, `src/preview.rs`) |
