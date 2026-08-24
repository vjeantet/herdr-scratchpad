//! Persistance : `scratchpad.txt`, en texte brut.
//!
//! Le fichier d'état *est* le texte. Pas de JSON : sans mode ni métadonnée à
//! ranger à côté, il ne servirait qu'à échapper le contenu et à le rendre
//! illisible au `cat`, inutilisable au `grep`.
//!
//! C'est ce qui fait du scratchpad un canal bidirectionnel : on écrit dans le
//! pane, un agent lit le fichier ; un agent écrit le fichier, le pane se
//! recharge (cf. [`Store::reload_if_changed`]).

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Nom du fichier d'état. Unique et global : le buffer n'est pas cloisonné par
/// workspace, contrairement à `herdr-notes`.
const STATE_FILE: &str = "scratchpad.txt";

/// Nom du fichier qui retient la dernière cible d'envoi.
///
/// Un fichier **voisin**, et non une ligne dans `scratchpad.txt` : celui-ci
/// doit rester du texte nu, c'est tout le contrat du canal bidirectionnel
/// (§8 du DESIGN).
const TARGET_FILE: &str = "target.txt";

/// Emplacement du fichier d'état.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Store {
    path: PathBuf,
    /// Dernier mtime **écrit ou lu par nous**, pour distinguer nos propres
    /// écritures de celles d'un tiers.
    seen: Option<SystemTime>,
}

/// Répertoire d'état fourni par herdr, ou repli sur le répertoire de config.
///
/// Hors herdr (`HERDR_PLUGIN_STATE_DIR` absent), le pane doit rester
/// utilisable : on tombe sur `<config>/herdr/scratchpad/`.
fn state_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("HERDR_PLUGIN_STATE_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    config_dir().map(|base| base.join("herdr").join("scratchpad"))
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

impl Store {
    /// Résout l'emplacement depuis l'environnement.
    pub fn from_env() -> Option<Self> {
        Some(Self::at(state_dir()?.join(STATE_FILE)))
    }

    /// Emplacement explicite — utilisé par les tests, qui ne doivent jamais
    /// dépendre de l'environnement réel.
    pub fn at(path: PathBuf) -> Self {
        Self { path, seen: None }
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
    /// Le nom du temporaire porte le **pid** : le design autorise plusieurs
    /// panes ouverts (§9), donc plusieurs écrivains sur ce même fichier. Un
    /// nom fixe les ferait se piétiner.
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

/// Dernière cible d'envoi mémorisée : *(libellé de workspace, agent)*.
///
/// Volontairement **pas** un `pane_id` : celui-ci ne survit pas à un
/// redémarrage de herdr, alors que cette paire se retrouve dans un
/// `agent.list` neuf.
pub fn load_target() -> Option<(String, String)> {
    read_target(&state_dir()?.join(TARGET_FILE))
}

/// Retient la cible. Un échec est silencieux : perdre cette mémoire ne coûte
/// qu'un cyclage de plus au prochain démarrage.
pub fn save_target(workspace_label: &str, agent: &str) {
    // Le séparateur est une tabulation : un libellé qui en contiendrait une
    // rendrait le fichier ambigu, on préfère ne rien écrire.
    if [workspace_label, agent]
        .iter()
        .any(|s| s.is_empty() || s.contains('\t') || s.contains('\n'))
    {
        return;
    }
    if let Some(dir) = state_dir() {
        let _ = write_target(&dir.join(TARGET_FILE), workspace_label, agent);
    }
}

fn read_target(path: &Path) -> Option<(String, String)> {
    let raw = std::fs::read_to_string(path).ok()?;
    let (label, agent) = raw.trim_end_matches('\n').split_once('\t')?;
    if label.is_empty() || agent.is_empty() {
        return None;
    }
    Some((label.to_owned(), agent.to_owned()))
}

/// Même écriture atomique que l'état : deux panes peuvent mémoriser en même
/// temps, et le temporaire porte déjà le pid.
fn write_target(path: &Path, workspace_label: &str, agent: &str) -> std::io::Result<()> {
    Store::at(path.to_owned()).save(&format!("{workspace_label}\t{agent}"))
}

/// Chemin de l'instantané d'export.
///
/// Fixe et écrasé : c'est une **adresse**, que les scripts et les agents
/// peuvent lire sans qu'on la leur donne. `/tmp` est vidé au reboot, sans
/// importance — la vraie persistance est le fichier d'état.
pub fn export_path() -> PathBuf {
    std::env::temp_dir().join("herdr-scratchpad.txt")
}

/// Écrit l'instantané. Même atomicité que l'état : un agent peut être en train
/// de le lire.
pub fn export(text: &str) -> std::io::Result<PathBuf> {
    let path = export_path();
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

        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("tmp"))
            .collect();
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

    #[test]
    fn export_path_is_stable_across_calls() {
        assert_eq!(export_path(), export_path());
        assert!(export_path().ends_with("herdr-scratchpad.txt"));
    }
    #[test]
    fn la_cible_memorisee_fait_un_aller_retour() {
        let path = scratch("target").join(TARGET_FILE);
        write_target(&path, "wdv", "claude").unwrap();
        assert_eq!(
            read_target(&path),
            Some(("wdv".to_owned(), "claude".to_owned()))
        );
    }

    #[test]
    fn une_cible_absente_se_lit_comme_rien() {
        let path = scratch("notarget").join(TARGET_FILE);
        assert_eq!(read_target(&path), None);
    }

    #[test]
    fn une_cible_illisible_se_lit_comme_rien() {
        let dir = scratch("badtarget");
        let path = dir.join(TARGET_FILE);
        std::fs::write(&path, "pas de tabulation ici").unwrap();
        assert_eq!(read_target(&path), None, "sans separateur, rien a lire");

        std::fs::write(&path, "\tclaude").unwrap();
        assert_eq!(read_target(&path), None, "un libelle vide ne designe rien");
    }

    /// La cible ne doit jamais atterrir dans le fichier de texte : celui-ci
    /// reste du texte nu, lisible au `cat` (§8 du DESIGN).
    #[test]
    fn la_cible_vit_dans_son_propre_fichier() {
        assert_ne!(TARGET_FILE, STATE_FILE);
    }
}
