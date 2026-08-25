//! Persistance : `scratchpad-<tab_id>.txt`, en texte brut.
//!
//! Le fichier d'état *est* le texte. Pas de JSON : sans mode ni métadonnée à
//! ranger à côté, il ne servirait qu'à échapper le contenu et à le rendre
//! illisible au `cat`, inutilisable au `grep`.
//!
//! C'est ce qui fait du scratchpad un canal bidirectionnel : on écrit dans le
//! pane, un agent lit le fichier ; un agent écrit le fichier, le pane se
//! recharge (cf. [`Store::reload_if_changed`]).
//!
//! Le nom porte le `tab_id` : un buffer par tab, pas un buffer global (§8, §9
//! du DESIGN). L'agent compose le chemin lui-même, `HERDR_TAB_ID` étant
//! injecté dans son pane comme dans le nôtre.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Nom du fichier d'état **sans clé de tab**.
///
/// Hors herdr il n'y a pas de `HERDR_TAB_ID`, donc pas de clé : le binaire
/// doit rester utilisable à la main, il retombe sur ce nom nu (§3.4 du plan).
/// C'est aussi le nom de l'ancien buffer global, que [`purge_legacy`] efface
/// — mais dans le state dir de herdr **seulement**.
const STATE_FILE: &str = "scratchpad.txt";

/// Préfixe et suffixe des buffers cloisonnés : `scratchpad-w1:t2.txt`.
const PREFIX: &str = "scratchpad-";
const SUFFIX: &str = ".txt";

/// Vestige de l'époque où la cible d'envoi se mémorisait sur disque.
///
/// La cible est maintenant locale à la tab et se déduit de `agent.list` : il
/// n'y a plus rien à retenir. Le nom ne survit que pour être supprimé.
const TARGET_FILE: &str = "target.txt";

/// Emplacement du fichier d'état.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Store {
    path: PathBuf,
    /// Dernier mtime **écrit ou lu par nous**, pour distinguer nos propres
    /// écritures de celles d'un tiers.
    seen: Option<SystemTime>,
}

/// Le state dir fourni par herdr, s'il y en a un.
///
/// Public parce que la purge des vestiges y est confinée : l'ancien buffer
/// global n'a jamais existé ailleurs (§3.5 du plan).
pub fn herdr_state_dir() -> Option<PathBuf> {
    std::env::var_os("HERDR_PLUGIN_STATE_DIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
}

/// Répertoire de repli, hors herdr : `<config>/herdr/scratchpad/`.
fn fallback_dir() -> Option<PathBuf> {
    config_dir().map(|base| base.join("herdr").join("scratchpad"))
}

/// Répertoire d'état fourni par herdr, ou repli sur le répertoire de config.
fn state_dir() -> Option<PathBuf> {
    herdr_state_dir().or_else(fallback_dir)
}

/// Les répertoires susceptibles de contenir des buffers : celui de herdr et
/// le repli. Sans doublon — hors herdr, les deux se confondent.
///
/// C'est la liste que balaie le ménage des orphelins (§3.6 du plan) : un
/// buffer écrit dans le repli avant que le plugin ne soit installé y reste
/// sinon indéfiniment.
pub fn state_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in [herdr_state_dir(), fallback_dir()].into_iter().flatten() {
        if !out.contains(&dir) {
            out.push(dir);
        }
    }
    out
}

fn config_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let base = std::env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
    base
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

/// Nom du fichier de `tab_id`, ou nom nu quand il n'y a pas de tab.
fn file_name(tab_id: Option<&str>) -> String {
    match tab_id {
        Some(id) => format!("{PREFIX}{id}{SUFFIX}"),
        None => STATE_FILE.to_owned(),
    }
}

/// Le `tab_id` que porte un nom de fichier, s'il en porte un.
///
/// `scratchpad.txt` rend `None` : le buffer sans clé n'est jamais candidat au
/// ménage. Les temporaires d'écriture (`scratchpad-w1:t1.tmp.42`) non plus,
/// faute du suffixe.
fn tab_id_of(name: &str) -> Option<&str> {
    name.strip_prefix(PREFIX)?
        .strip_suffix(SUFFIX)
        .filter(|id| !id.is_empty())
}

impl Store {
    /// Résout l'emplacement depuis l'environnement, pour la tab donnée.
    pub fn from_env(tab_id: Option<&str>) -> Option<Self> {
        Some(Self::at(state_dir()?.join(file_name(tab_id))))
    }

    /// Emplacement explicite — utilisé par les tests, qui ne doivent jamais
    /// dépendre de l'environnement réel.
    pub fn at(path: PathBuf) -> Self {
        Self { path, seen: None }
    }

    /// Le fichier de ce store. Le ménage s'en sert pour ne pas se supprimer.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Lit le texte. Un fichier absent ou illisible rend une chaîne vide.
    ///
    /// Volontairement indulgent : rien ici ne doit pouvoir coincer le pane. Un
    /// fichier effacé à la main, un disque plein, un `rename` à moitié fait —
    /// le pane s'ouvre quand même, vide.
    pub fn load(&mut self) -> String {
        self.seen = mtime(&self.path);
        std::fs::read_to_string(&self.path).unwrap_or_default()
    }

    /// Écrit le texte de façon atomique : temporaire + `fsync` + `rename`.
    ///
    /// Le `fsync` **avant** le `rename` est la partie qui compte : sans lui,
    /// une coupure peut rendre le rename durable avant les données, laissant
    /// un fichier vide que le chargement indulgent transformerait en buffer
    /// vide — c'est-à-dire une perte silencieuse.
    ///
    /// Le nom du temporaire porte le **pid**. Depuis le cloisonnement par tab,
    /// deux écrivains sur un même fichier ne devraient plus exister ; la
    /// ceinture ne coûte rien et couvre encore le repli sans clé, où plusieurs
    /// binaires lancés à la main partagent bien un fichier.
    pub fn save(&mut self, text: &str) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self
            .path
            .with_extension(format!("tmp.{}", std::process::id()));

        let written = std::fs::File::create(&tmp).and_then(|mut f| {
            use std::io::Write;
            f.write_all(text.as_bytes())?;
            f.sync_all()
        });

        // On ne renomme que si l'écriture ET le fsync ont réussi : une
        // écriture ratée ne doit jamais pouvoir remplacer un bon fichier par
        // un fichier tronqué.
        match written {
            Ok(()) => {
                std::fs::rename(&tmp, &self.path)?;
                self.seen = mtime(&self.path);
                Ok(())
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                Err(e)
            }
        }
    }

    /// Rend le texte si le fichier a changé depuis notre dernière lecture ou
    /// écriture, `None` sinon.
    ///
    /// C'est toute la surveillance de mtime : appelée à intervalle régulier,
    /// elle transforme le fichier en source de vérité partagée entre les panes
    /// ouverts et les agents qui écrivent dedans.
    ///
    /// L'appelant reste responsable de ne pas écraser des frappes non
    /// sauvegardées — un pane qui a du texte en attente ignore le rechargement.
    pub fn reload_if_changed(&mut self) -> Option<String> {
        let current = mtime(&self.path);
        if current == self.seen {
            return None;
        }
        self.seen = current;
        Some(std::fs::read_to_string(&self.path).unwrap_or_default())
    }
}

/// Supprime dans `dir` les vestiges du buffer global : le `scratchpad.txt`
/// **et** le `target.txt`.
///
/// Sèchement, sans sauvegarde ni migration : adopter l'ancien texte dans une
/// tab choisie arbitrairement recréerait exactement la surprise que le
/// cloisonnement supprime (§3.5 du plan).
///
/// À n'appeler que sur le **state dir de herdr**. Dans le répertoire de repli,
/// `scratchpad.txt` n'est pas un vestige : c'est le buffer d'un binaire lancé
/// à la main (§3.4) — voir [`purge_legacy_target`].
///
/// Idempotent et muet : une corvée ratée n'a rien à dire.
pub fn purge_legacy(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(STATE_FILE));
    purge_legacy_target(dir);
}

/// Supprime le seul `target.txt` de `dir`.
///
/// Il y en a un dans chacun des deux répertoires sur la machine de référence,
/// et aucun des deux ne sert plus à rien.
pub fn purge_legacy_target(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(TARGET_FILE));
}

/// Supprime dans `dir` les buffers dont la tab n'existe plus.
///
/// Les numéros de tab publics ne sont **jamais réutilisés** (vérifié dans
/// `herdr src/workspace.rs:1593`), donc un fichier dont la tab manque est
/// orphelin définitivement : aucune tab neuve n'héritera de son texte.
///
/// `live_tab_ids` arrive **déjà validée** : c'est l'appelant qui s'abstient
/// quand `tab.list` échoue ou répond une liste vide — une liste vide n'est
/// jamais une information, c'est une panne. Garder cette fonction bête est ce
/// qui la rend testable sans socket.
///
/// `own` n'est jamais supprimé, même si sa tab manquait de la liste.
pub fn sweep_orphans(dir: &Path, live_tab_ids: &[String], own: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path == own {
            continue;
        }
        let name = entry.file_name();
        let Some(tab_id) = name.to_str().and_then(tab_id_of) else {
            continue;
        };
        if !live_tab_ids.iter().any(|live| live == tab_id) {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Chemin de l'instantané d'export, cloisonné lui aussi.
///
/// C'est une **adresse** que les scripts et les agents composent sans qu'on la
/// leur donne — d'où le `tab_id`, qu'ils ont dans leur environnement. Un
/// chemin fixe ferait écraser silencieusement l'instantané d'une autre tab, au
/// seul endroit que personne ne surveille (§3.10 du plan).
pub fn export_path(tab_id: Option<&str>) -> PathBuf {
    let name = match tab_id {
        Some(id) => format!("herdr-scratchpad-{id}.txt"),
        None => "herdr-scratchpad.txt".to_owned(),
    };
    std::env::temp_dir().join(name)
}

/// Écrit l'instantané. Même atomicité que l'état : un agent peut être en train
/// de le lire.
pub fn export(text: &str, tab_id: Option<&str>) -> std::io::Result<PathBuf> {
    let path = export_path(tab_id);
    let mut store = Store::at(path.clone());
    store.save(text)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Répertoire jetable, sans dépendance de test externe.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("herdr-scratchpad-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Un fichier vide, juste pour le voir survivre ou disparaître.
    fn touch(path: &Path) {
        std::fs::write(path, "").unwrap();
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut out: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        out.sort();
        out
    }

    #[test]
    fn missing_file_loads_as_empty() {
        let mut store = Store::at(scratch("missing").join(STATE_FILE));
        assert_eq!(store.load(), "");
    }

    #[test]
    fn save_then_load_round_trips() {
        let mut store = Store::at(scratch("round").join(STATE_FILE));
        store.save("une ligne\net une autre").unwrap();
        assert_eq!(store.load(), "une ligne\net une autre");
    }

    #[test]
    fn save_creates_missing_directories() {
        let path = scratch("mkdir").join("a").join("b").join(STATE_FILE);
        let mut store = Store::at(path.clone());
        store.save("x").unwrap();
        assert!(path.exists());
    }

    #[test]
    fn state_file_is_plain_text_readable_by_anything() {
        let path = scratch("plain").join(STATE_FILE);
        let mut store = Store::at(path.clone());
        store.save("# titre\n\"guillemets\"\ttab").unwrap();

        // Aucun échappement : c'est le contrat du canal bidirectionnel.
        let raw = std::fs::read_to_string(&path).unwrap();
        assert_eq!(raw, "# titre\n\"guillemets\"\ttab");
    }

    #[test]
    fn save_leaves_no_temp_file_behind() {
        let dir = scratch("notmp");
        let mut store = Store::at(dir.join(STATE_FILE));
        store.save("x").unwrap();

        let leftovers: Vec<_> = names(&dir).into_iter().filter(|n| n.contains("tmp")).collect();
        assert!(leftovers.is_empty(), "temporaires restants : {leftovers:?}");
    }

    #[test]
    fn our_own_save_does_not_look_like_an_external_change() {
        let mut store = Store::at(scratch("self").join(STATE_FILE));
        store.load();
        store.save("écrit par nous").unwrap();
        assert_eq!(store.reload_if_changed(), None);
    }

    #[test]
    fn external_write_is_detected() {
        let path = scratch("external").join(STATE_FILE);
        let mut store = Store::at(path.clone());
        store.save("initial").unwrap();

        // Un agent écrit le fichier dans notre dos. Le mtime a une résolution
        // limitée : on force une valeur distincte plutôt que d'attendre.
        std::fs::write(&path, "écrit par un agent").unwrap();
        let bumped = SystemTime::now() + std::time::Duration::from_secs(2);
        let _ = std::fs::File::open(&path).map(|f| f.set_modified(bumped));

        assert_eq!(
            store.reload_if_changed().as_deref(),
            Some("écrit par un agent")
        );
    }

    #[test]
    fn detected_change_is_reported_once() {
        let path = scratch("once").join(STATE_FILE);
        let mut store = Store::at(path.clone());
        store.save("a").unwrap();

        std::fs::write(&path, "b").unwrap();
        let bumped = SystemTime::now() + std::time::Duration::from_secs(2);
        let _ = std::fs::File::open(&path).map(|f| f.set_modified(bumped));

        assert!(store.reload_if_changed().is_some());
        assert_eq!(store.reload_if_changed(), None, "pas de rechargement en boucle");
    }

    #[test]
    fn deleting_the_file_reads_as_empty_not_as_an_error() {
        let path = scratch("deleted").join(STATE_FILE);
        let mut store = Store::at(path.clone());
        store.save("du texte").unwrap();
        std::fs::remove_file(&path).unwrap();

        assert_eq!(store.reload_if_changed().as_deref(), Some(""));
    }

    // -- la clé de tab ----------------------------------------------------

    #[test]
    fn the_file_name_carries_the_tab_id() {
        assert_eq!(file_name(Some("w1:t2")), "scratchpad-w1:t2.txt");
    }

    /// Hors herdr il n'y a pas de tab : le binaire reste utilisable à la main,
    /// sur le nom nu.
    #[test]
    fn without_a_tab_id_the_name_has_no_suffix() {
        assert_eq!(file_name(None), STATE_FILE);
    }

    #[test]
    fn two_tabs_do_not_share_their_file() {
        assert_ne!(file_name(Some("w1:t1")), file_name(Some("w1:t2")));
    }

    #[test]
    fn the_tab_id_can_be_read_back_from_the_file_name() {
        assert_eq!(tab_id_of("scratchpad-w1:t2.txt"), Some("w1:t2"));
    }

    /// Le fichier sans clé n'appartient à aucune tab : il ne doit jamais
    /// devenir candidat au ménage.
    #[test]
    fn the_unsuffixed_file_carries_no_tab_id() {
        assert_eq!(tab_id_of(STATE_FILE), None);
        assert_eq!(tab_id_of("scratchpad-.txt"), None);
        assert_eq!(tab_id_of("scratchpad-w1:t1.tmp.42"), None, "un temporaire d'écriture");
    }

    // -- purge des vestiges -----------------------------------------------

    #[test]
    fn purge_legacy_deletes_the_old_global_buffer_and_the_stored_target() {
        let dir = scratch("purge");
        touch(&dir.join(STATE_FILE));
        touch(&dir.join(TARGET_FILE));
        touch(&dir.join("scratchpad-w1:t1.txt"));

        purge_legacy(&dir);
        assert_eq!(
            names(&dir),
            vec!["scratchpad-w1:t1.txt"],
            "les buffers cloisonnés ne sont pas des vestiges"
        );
    }

    #[test]
    fn purge_legacy_on_an_empty_directory_does_not_panic() {
        purge_legacy(&scratch("purgevide"));
    }

    /// Dans le repli, `scratchpad.txt` est le buffer d'un binaire lancé à la
    /// main : seule la cible mémorisée y est un vestige.
    #[test]
    fn purge_legacy_target_spares_the_unsuffixed_buffer() {
        let dir = scratch("purgetarget");
        touch(&dir.join(STATE_FILE));
        touch(&dir.join(TARGET_FILE));

        purge_legacy_target(&dir);
        assert_eq!(names(&dir), vec![STATE_FILE]);
    }

    // -- ménage des orphelins ---------------------------------------------

    #[test]
    fn sweep_deletes_a_buffer_whose_tab_is_gone() {
        let dir = scratch("sweep");
        let own = dir.join("scratchpad-w1:t1.txt");
        touch(&own);
        touch(&dir.join("scratchpad-w9:t9.txt"));

        sweep_orphans(&dir, &["w1:t1".to_owned()], &own);
        assert_eq!(names(&dir), vec!["scratchpad-w1:t1.txt"]);
    }

    /// Le nôtre survit quoi qu'il arrive : une tab absente de la liste au
    /// démarrage ne doit pas nous faire effacer le texte qu'on vient d'ouvrir.
    #[test]
    fn sweep_never_deletes_its_own() {
        let dir = scratch("sweepown");
        let own = dir.join("scratchpad-w1:t1.txt");
        touch(&own);

        sweep_orphans(&dir, &["w2:t2".to_owned()], &own);
        assert_eq!(names(&dir), vec!["scratchpad-w1:t1.txt"]);
    }

    #[test]
    fn sweep_leaves_the_unsuffixed_file_alone() {
        let dir = scratch("sweepnu");
        touch(&dir.join(STATE_FILE));

        sweep_orphans(&dir, &["w1:t1".to_owned()], &dir.join("ailleurs.txt"));
        assert_eq!(names(&dir), vec![STATE_FILE]);
    }

    #[test]
    fn sweep_on_a_missing_directory_does_not_panic() {
        let dir = scratch("sweepabsent").join("jamais-cree");
        sweep_orphans(&dir, &["w1:t1".to_owned()], &dir.join("x.txt"));
    }

    // -- export -----------------------------------------------------------

    #[test]
    fn export_path_is_stable_across_calls() {
        assert_eq!(export_path(Some("w1:t1")), export_path(Some("w1:t1")));
    }

    #[test]
    fn the_export_path_carries_the_tab_id() {
        assert!(export_path(Some("w1:t2")).ends_with("herdr-scratchpad-w1:t2.txt"));
    }

    /// Deux tabs qui exportent ne doivent pas s'écraser l'une l'autre.
    #[test]
    fn two_tabs_do_not_export_to_the_same_file() {
        assert_ne!(export_path(Some("w1:t1")), export_path(Some("w1:t2")));
    }

    #[test]
    fn without_a_tab_id_the_export_keeps_the_bare_path() {
        assert!(export_path(None).ends_with("herdr-scratchpad.txt"));
    }
}
