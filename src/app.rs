//! L'état du pane et sa machine à événements.

use std::path::Path;
use std::time::{Duration, Instant};

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use ratatui::Frame;

use crate::agents::{self, Target};
use crate::buffer::{Buffer, PAGE_FALLBACK};
use crate::clipboard::{self, CopyError, MAX_CLIPBOARD_BYTES};
use crate::ipc::{Herdr, Socket};
use crate::state::{self, Store};
use crate::ui;

/// La tab des bancs d'essai. Un pane sans tab n'aurait aucune cible : les
/// tests d'envoi n'auraient alors rien à vérifier.
#[cfg(test)]
const TEST_TAB: &str = "w1:t1";

/// Sauvegarde ~500 ms après la dernière frappe.
///
/// Plus court que les 2 s de `herdr-notes` : ici le fichier d'état sert de
/// canal vers les agents, donc la fraîcheur compte. Une sauvegarde est une
/// écriture atomique de quelques kilo-octets, on peut se le permettre.
const AUTOSAVE_AFTER: Duration = Duration::from_millis(500);
/// Ré-estampillage de vivacité. Le lanceur déclare mort au-delà de 20 s.
const HEARTBEAT_EVERY: Duration = Duration::from_secs(5);
/// Fréquence de surveillance du fichier d'état.
const WATCH_EVERY: Duration = Duration::from_millis(700);
/// Durée d'affichage d'un message avant retour aux boutons.
const STATUS_FOR: Duration = Duration::from_secs(3);
/// Lignes parcourues par cran de molette.
const WHEEL_STEP: usize = 3;
/// Rafraîchissement de la liste des agents.
///
/// L'affichage n'a besoin d'être qu'à peu près à jour : la cible est de toute
/// façon **re-résolue au moment de l'envoi** (cf. [`App::send`]). Deux
/// secondes et demie suffisent donc largement, pour un seul appel socket —
/// le filtre par tab se lit dans `agent.list`, qui porte déjà le `tab_id`.
const TARGET_REFRESH: Duration = Duration::from_millis(2500);

/// Les cinq commandes. Rien d'autre — il n'y a pas de touche pour quitter :
/// `prefix+a` referme le pane, geste symétrique de celui qui l'a ouvert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Dépose le texte chez un agent, **sans le soumettre**, et vide.
    Send,
    Copy,
    Clear,
    Export,
    Undo,
}

/// Ce que déclenche une entrée de la barre du bas.
///
/// La zone cible n'est pas une commande : elle ne touche pas au texte, elle
/// choisit à qui l'envoyer. Les confondre ferait passer un cyclage pour une
/// action sur le buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Command(Command),
    CycleTarget,
}

pub struct App {
    buf: Buffer,
    store: Option<Store>,
    /// Case de secours du vidage, à une seule place.
    ///
    /// Une suffit : le regret d'un vidage se manifeste dans les trois
    /// secondes, pas trois vidages plus tard.
    stash: Option<String>,
    status: Option<(String, Instant)>,
    scroll: usize,
    /// Géométrie du dernier rendu, rejouée au clic suivant.
    buttons: Vec<(Action, Rect)>,
    body: Rect,
    total_rows: usize,

    dirty: bool,
    last_edit: Instant,
    last_beat: Instant,
    last_watch: Instant,
    last_targets: Instant,
    pane_id: Option<String>,

    /// Le serveur herdr, derrière une indirection pour que les tests puissent
    /// lui substituer des réponses figées.
    herdr: Box<dyn Herdr>,
    /// La tab où ce pane est né, telle que herdr la lui a dite au spawn.
    ///
    /// C'est **la** clé du plugin : elle nomme le fichier d'état, borne les
    /// cibles d'envoi et suffixe l'export. Figée : un pane déplacé vers une
    /// autre tab (`pane.move`) garde donc son buffer et ses cibles d'origine
    /// — limite connue, assumée (§9 du DESIGN).
    tab_id: Option<String>,
    /// Agents de cette tab, rafraîchis toutes les [`TARGET_REFRESH`].
    targets: Vec<Target>,
    /// Index de la cible dans `targets`.
    target: Option<usize>,
}

impl App {
    pub fn new() -> Self {
        // La tab avant tout le reste : c'est elle qui nomme le fichier.
        let tab_id = env("HERDR_TAB_ID");
        let mut store = Store::from_env(tab_id.as_deref());
        let herdr: Box<dyn Herdr> = Box::new(Socket);

        // Le ménage passe **avant** le chargement : ce qu'on efface ne doit
        // jamais être ce qu'on est en train d'ouvrir.
        purge_legacy();
        if let Some(store) = store.as_ref() {
            sweep_orphans(herdr.as_ref(), store.path());
        }

        let text = store.as_mut().map(Store::load).unwrap_or_default();

        let mut app = Self {
            buf: Buffer::from_text(&text),
            store,
            stash: None,
            status: None,
            scroll: 0,
            buttons: Vec::new(),
            body: Rect::default(),
            total_rows: 0,
            dirty: false,
            last_edit: Instant::now(),
            last_beat: Instant::now(),
            last_watch: Instant::now(),
            last_targets: Instant::now(),
            pane_id: env("HERDR_PANE_ID"),
            herdr,
            tab_id,
            targets: Vec::new(),
            target: None,
        };
        app.stamp();
        // La barre doit être juste dès le premier rendu : sans ça `^E`
        // apparaîtrait deux secondes et demie après l'ouverture, en décalant
        // ce qui est déjà sous le doigt.
        app.refresh_targets();
        app
    }

    /// Construit une instance sans toucher au disque ni à l'environnement.
    #[cfg(test)]
    fn headless(text: &str) -> Self {
        Self::with_herdr(text, Box::new(crate::ipc::Socket))
    }

    /// Idem, avec un serveur herdr substitué.
    #[cfg(test)]
    fn with_herdr(text: &str, herdr: Box<dyn Herdr>) -> Self {
        Self {
            buf: Buffer::from_text(text),
            store: None,
            stash: None,
            status: None,
            scroll: 0,
            buttons: Vec::new(),
            body: Rect::default(),
            total_rows: 0,
            dirty: false,
            last_edit: Instant::now(),
            last_beat: Instant::now(),
            last_watch: Instant::now(),
            last_targets: Instant::now(),
            pane_id: None,
            herdr,
            // Les tests vivent dans une tab, comme un vrai pane : sans clé,
            // il n'y aurait aucune cible et rien à vérifier.
            tab_id: Some(TEST_TAB.to_owned()),
            targets: Vec::new(),
            target: None,
        }
    }

    // -- rendu ------------------------------------------------------------

    pub fn draw(&mut self, frame: &mut Frame) {
        let status = self
            .status
            .as_ref()
            .filter(|(_, at)| at.elapsed() < STATUS_FOR)
            .map(|(text, _)| text.as_str());
        // Emprunt de champ à champ : `current_target` emprunterait `self`
        // entier, ce que `&mut self.scroll` interdit dans le même appel.
        let targets = ui::Targets {
            current: self.target.and_then(|i| self.targets.get(i)),
            count: self.targets.len(),
        };

        let geom = ui::draw(
            frame,
            self.buf.lines(),
            self.buf.cursor(),
            &mut self.scroll,
            status,
            self.buf.is_empty(),
            targets,
        );
        self.buttons = geom.buttons;
        self.body = geom.body;
        self.total_rows = geom.total_rows;
    }

    // -- entrées ----------------------------------------------------------

    pub fn on_key(&mut self, key: KeyEvent) {
        // Windows livre les répétitions et les relâchements ; seules les
        // pressions nous intéressent.
        if key.kind == KeyEventKind::Release {
            return;
        }

        // AltGr arrive de Windows en CONTROL|ALT sur un caractère ordinaire :
        // sans cette garde, taper `@` ou `#` déclencherait une commande.
        let altgr = key.modifiers.contains(KeyModifiers::CONTROL)
            && key.modifiers.contains(KeyModifiers::ALT);

        if key.modifiers.contains(KeyModifiers::CONTROL) && !altgr {
            match key.code {
                KeyCode::Char('e') | KeyCode::Char('E') => self.run(Command::Send),
                KeyCode::Char('n') | KeyCode::Char('N') => self.cycle_target(),
                KeyCode::Char('c') | KeyCode::Char('C') => self.run(Command::Copy),
                KeyCode::Char('l') | KeyCode::Char('L') => self.run(Command::Clear),
                KeyCode::Char('s') | KeyCode::Char('S') => self.run(Command::Export),
                KeyCode::Char('z') | KeyCode::Char('Z') => self.run(Command::Undo),
                // Les sauts de mot ne portent pas de lettre : ils ne disputent
                // rien aux commandes, contrairement au readline écarté par le
                // design (`Ctrl+A/E/K/W/U`).
                KeyCode::Left => self.buf.word_left(),
                KeyCode::Right => self.buf.word_right(),
                KeyCode::Home => self.buf.cursor_to_start(),
                KeyCode::End => self.buf.cursor_to_end(),
                // `Ctrl+Backspace` arrive selon le terminal soit tel quel,
                // soit en `^H` — c'est-à-dire `Ctrl+H` (crossterm
                // `event/sys/unix/parse.rs:106`). Les deux formes valent la
                // même chose, et `h` n'est pris par aucune commande.
                KeyCode::Backspace | KeyCode::Char('h') | KeyCode::Char('H') => {
                    self.buf.delete_word_left();
                    self.touch();
                }
                // Toute autre combinaison `Ctrl` est avalée : la laisser
                // tomber dans le `match` ordinaire ferait *taper* sa lettre.
                _ => {}
            }
            return;
        }

        let page = if self.body.height > 0 {
            self.body.height as usize
        } else {
            PAGE_FALLBACK
        };

        match key.code {
            KeyCode::Char(c) => {
                self.buf.insert_char(c);
                self.touch();
            }
            KeyCode::Enter => {
                self.buf.insert_newline();
                self.touch();
            }
            KeyCode::Backspace => {
                self.buf.backspace();
                self.touch();
            }
            KeyCode::Delete => {
                self.buf.delete();
                self.touch();
            }
            KeyCode::Left => self.buf.left(),
            KeyCode::Right => self.buf.right(),
            KeyCode::Up => self.buf.up(1),
            KeyCode::Down => self.buf.down(1),
            KeyCode::Home => self.buf.home(),
            KeyCode::End => self.buf.end(),
            KeyCode::PageUp => self.buf.up(page),
            KeyCode::PageDown => self.buf.down(page),
            _ => {}
        }
    }

    /// Collage livré d'un bloc par le *bracketed paste*.
    pub fn on_paste(&mut self, text: &str) {
        self.buf.insert_str(text);
        self.touch();
    }

    pub fn on_mouse(&mut self, ev: MouseEvent) {
        // `Shift`+souris appartient au terminal : c'est ainsi que herdr
        // préserve la sélection native malgré la capture. Ne jamais y toucher.
        if ev.modifiers.contains(KeyModifiers::SHIFT) {
            return;
        }
        let pos = Position { x: ev.column, y: ev.row };

        match ev.kind {
            // Les boutons agissent à la pression : ils ne peuvent pas démarrer
            // de glisser, donc rien ne justifie d'attendre le relâchement.
            MouseEventKind::Down(MouseButton::Left) => {
                let hit = self
                    .buttons
                    .iter()
                    .find(|(_, r)| r.contains(pos))
                    .map(|(action, _)| *action);
                match hit {
                    Some(Action::Command(command)) => self.run(command),
                    Some(Action::CycleTarget) => self.cycle_target(),
                    // Hors de la barre : le clic pose le curseur. La position
                    // est calculée avant d'emprunter le buffer en écriture.
                    None => {
                        let at = ui::position_to_cursor(
                            self.buf.lines(),
                            self.body,
                            self.scroll,
                            pos,
                        );
                        if let Some((row, col)) = at {
                            self.buf.set_cursor(row, col);
                        }
                    }
                }
            }
            MouseEventKind::ScrollUp => self.scroll = self.scroll.saturating_sub(WHEEL_STEP),
            MouseEventKind::ScrollDown => {
                let max = self
                    .total_rows
                    .saturating_sub(self.body.height as usize);
                self.scroll = (self.scroll + WHEEL_STEP).min(max);
            }
            _ => {}
        }
    }

    /// Le pane perd le focus : on sauvegarde tout de suite.
    ///
    /// `herdr pane close` tue le processus sans signal — sans ce point de
    /// sauvegarde, une frappe des 500 dernières millisecondes serait perdue.
    pub fn on_focus_lost(&mut self) {
        self.flush();
    }

    // -- commandes --------------------------------------------------------

    fn run(&mut self, command: Command) {
        match command {
            Command::Send => self.send(),
            Command::Copy => self.copy(),
            Command::Clear => self.clear(),
            Command::Export => self.export(),
            Command::Undo => self.undo(),
        }
    }

    fn copy(&mut self) {
        let text = self.buf.text();
        match clipboard::copy(&text) {
            Ok(len) => self.say(format!("copied · {}", human(len))),
            Err(CopyError::Empty) => self.say("nothing to copy".into()),
            Err(CopyError::TooLarge { len }) => self.say(format!(
                "{} > {} — ^S writes the file",
                human(len),
                human(MAX_CLIPBOARD_BYTES)
            )),
            Err(CopyError::Io(e)) => self.say(format!("copy failed: {e}")),
        }
    }

    fn clear(&mut self) {
        let text = self.buf.text();
        if text.is_empty() {
            self.say("already empty".into());
            return;
        }
        self.stash_and_clear(text);
        self.say("cleared · ^Z undoes".into());
    }

    /// Déplace le texte dans la case de secours et vide le buffer.
    ///
    /// Un seul chemin pour `Ctrl+L` et pour l'envoi : un dépôt est un
    /// *déplacement*, donc il doit être rattrapable par `Ctrl+Z` exactement
    /// comme un vidage.
    fn stash_and_clear(&mut self, text: String) {
        self.stash = Some(text);
        self.buf = Buffer::default();
        self.scroll = 0;
        self.touch();
        // Vider est destructif : on l'écrit sur le disque tout de suite plutôt
        // que d'attendre la temporisation.
        self.flush();
    }

    fn undo(&mut self) {
        match self.stash.take() {
            Some(text) => {
                self.buf = Buffer::from_text(&text);
                self.touch();
                self.flush();
                self.say("restored".into());
            }
            None => self.say("nothing to restore".into()),
        }
    }

    fn export(&mut self) {
        let text = self.buf.text();
        match state::export(&text, self.tab_id.as_deref()) {
            Ok(path) => self.say(path.display().to_string()),
            Err(e) => self.say(format!("export failed: {e}")),
        }
    }

    /// Dépose le texte chez l'agent visé, puis vide.
    ///
    /// L'ordre compte : on envoie **puis** on vide. Un échec laisse donc le
    /// texte exactement où il était — c'est ce qui rend l'erreur sans
    /// conséquence, et c'est ce qui remplace toute confirmation.
    fn send(&mut self) {
        let text = self.buf.text();
        if text.is_empty() {
            self.say("nothing to emit".into());
            return;
        }

        // Ce que la barre affichait au moment du clic. C'est **cette** cible
        // qu'on envoie, pas celle qu'un rafraîchissement choisirait à sa
        // place : le garde-fou est l'affichage, il perdrait tout son sens si
        // la destination pouvait changer entre la lecture et l'appui.
        let Some(intended) = self.current_target().map(|t| t.pane_id.clone()) else {
            self.say("no agent".into());
            return;
        };

        // Re-résolution juste avant d'agir : l'affichage n'a besoin d'être
        // qu'à peu près à jour, l'action doit être exactement juste.
        self.refresh_targets();
        let Some(target) = self.targets.iter().find(|t| t.pane_id == intended).cloned() else {
            self.say("agent not found".into());
            return;
        };

        if let Err(e) = self.herdr.send_input(&target.pane_id, &text) {
            self.say(format!("emit failed: {e}"));
            return;
        }

        self.stash_and_clear(text);
        self.say(format!("emitted → {} · ^Z undoes", target.agent));

        // Basculer chez l'agent est la fin du geste : déposer, relire,
        // soumettre. Sans ça il faudrait aller chercher le pane à la main,
        // c'est-à-dire exactement le travail que le bouton devait épargner.
        //
        // En dernier, et après le vidage : le focus perdu déclenche une
        // sauvegarde immédiate côté TUI, elle doit trouver le buffer déjà vide.
        self.herdr.focus_agent(&target.pane_id);
    }

    /// La cible affichée, s'il y en a une.
    fn current_target(&self) -> Option<&Target> {
        self.target.and_then(|i| self.targets.get(i))
    }

    /// Cible suivante, en boucle. Ne touche pas au texte.
    ///
    /// **Aucun message de retour**, contrairement aux autres commandes : la
    /// zone cible affiche déjà la destination en permanence, c'est tout son
    /// rôle. Un message la dirait deux fois — et surtout, un message prend la
    /// place des boutons pendant trois secondes, donc masquerait la zone qu'on
    /// est en train de manipuler et rendrait le cyclage au clic impraticable.
    /// En dessous de deux cibles, il n'y a nulle part où aller : la touche et
    /// le clic sont **inertes**, sans message — cohérent avec le reste du
    /// cyclage, qui n'a jamais de retour.
    fn cycle_target(&mut self) {
        if self.targets.len() < 2 {
            return;
        }
        self.target = agents::next(&self.targets, self.target);
    }

    fn say(&mut self, message: String) {
        self.status = Some((message, Instant::now()));
    }

    // -- horloges ---------------------------------------------------------

    fn touch(&mut self) {
        self.dirty = true;
        self.last_edit = Instant::now();
    }

    /// Sauvegarde temporisée.
    pub fn maybe_flush(&mut self) {
        if self.dirty && self.last_edit.elapsed() >= AUTOSAVE_AFTER {
            self.flush();
        }
    }

    pub fn flush(&mut self) {
        if !self.dirty {
            return;
        }
        let text = self.buf.text();
        if let Some(store) = self.store.as_mut()
            && let Err(e) = store.save(&text)
        {
            self.status = Some((format!("save failed: {e}"), Instant::now()));
            return;
        }
        self.dirty = false;
    }

    /// Recharge si le fichier a bougé sous nos pieds.
    ///
    /// La garde `dirty` est la règle qui rend le multi-pane sûr : un pane qui
    /// a des frappes en attente ne se laisse jamais écraser.
    pub fn maybe_reload(&mut self) {
        if self.last_watch.elapsed() < WATCH_EVERY {
            return;
        }
        self.last_watch = Instant::now();
        if self.dirty {
            return;
        }
        if let Some(store) = self.store.as_mut()
            && let Some(text) = store.reload_if_changed()
            && text != self.buf.text()
        {
            self.buf.replace_preserving_cursor(&text);
        }
    }

    /// Rafraîchit la liste des agents à intervalle régulier.
    pub fn maybe_refresh_targets(&mut self) {
        if self.last_targets.elapsed() < TARGET_REFRESH {
            return;
        }
        self.last_targets = Instant::now();
        self.refresh_targets();
    }

    /// Re-demande la liste des agents et **préserve la cible sélectionnée**.
    ///
    /// La préservation se fait par `pane_id` : sans elle, un agent qui
    /// apparaît ailleurs dans la liste ferait glisser la sélection sous les
    /// doigts de l'utilisateur entre deux rafraîchissements.
    ///
    /// Une réponse manquante laisse la liste précédente en place : un socket
    /// qui hoquette ne doit pas faire disparaître `^E` de la barre une
    /// demi-seconde, en décalant ce qui est sous le doigt.
    fn refresh_targets(&mut self) {
        let Some(agents_json) = self.herdr.agent_list() else {
            return;
        };
        let kept = self.current_target().map(|t| t.pane_id.clone());
        self.targets = agents::targets(
            &agents_json,
            self.tab_id.as_deref(),
            self.pane_id.as_deref(),
        );
        // À défaut, la première : l'ordre est stable, donc « la première »
        // désigne toujours le même agent d'un rafraîchissement à l'autre.
        self.target = kept
            .and_then(|id| self.targets.iter().position(|t| t.pane_id == id))
            .or_else(|| (!self.targets.is_empty()).then_some(0));
    }

    pub fn heartbeat(&mut self) {
        if self.last_beat.elapsed() < HEARTBEAT_EVERY {
            return;
        }
        self.last_beat = Instant::now();
        self.stamp();
    }

    fn stamp(&self) {
        if let Some(id) = self.pane_id.as_deref() {
            crate::ipc::stamp(id);
        }
    }

    /// Dernière sauvegarde en sortie.
    pub fn finalize(&mut self) {
        self.flush();
    }
}

/// Une variable d'environnement non vide.
fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

/// Efface les vestiges de l'époque du buffer global (§3.5 du plan).
///
/// L'ancien `scratchpad.txt` ne part que du state dir de herdr : dans le
/// répertoire de repli, ce nom est celui du buffer d'un binaire lancé à la
/// main, qui n'a jamais été global. Les deux `target.txt`, eux, partent.
fn purge_legacy() {
    for dir in state::state_dirs() {
        state::purge_legacy_target(&dir);
    }
    if let Some(dir) = state::herdr_state_dir() {
        state::purge_legacy(&dir);
    }
}

/// Supprime les buffers dont la tab n'existe plus, dans les deux répertoires.
///
/// Au démarrage seulement, jamais dans le rafraîchissement : c'est une corvée,
/// pas une horloge.
fn sweep_orphans(herdr: &dyn Herdr, own: &Path) {
    let Some(live) = live_tabs(herdr) else {
        return;
    };
    for dir in state::state_dirs() {
        state::sweep_orphans(&dir, &live, own);
    }
}

/// Les tabs vivantes, ou `None` pour s'abstenir de tout ménage.
///
/// Deux abstentions, une seule raison : **une liste vide n'est jamais une
/// information**. Un serveur muet ou une réponse en erreur rendraient tous les
/// buffers orphelins d'un coup, et le ménage effacerait le travail de toutes
/// les tabs pour cause de panne.
fn live_tabs(herdr: &dyn Herdr) -> Option<Vec<String>> {
    let json = herdr.tab_list()?;
    let live = agents::live_tab_ids(&json);
    (!live.is_empty()).then_some(live)
}

/// Taille lisible. Les tailles qui comptent ici vont de l'octet au méga-octet.
fn human(bytes: usize) -> String {
    const KB: usize = 1024;
    const MB: usize = KB * KB;
    if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    /// `Ctrl` sur une touche qui n'est pas une lettre — flèches, `Backspace`.
    fn ctrl_code(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn text_of(app: &App) -> String {
        app.buf.text()
    }

    /// Un serveur herdr en carton : des réponses figées, et la trace de ce
    /// qui lui a été déposé. Aucun test unitaire n'ouvre de socket.
    #[derive(Default)]
    struct FakeHerdr {
        agents: String,
        tabs: String,
        /// Message d'erreur à rendre au dépôt, si on veut simuler un échec.
        refuse: Option<String>,
        sent: std::cell::RefCell<Vec<(String, String)>>,
        focused: std::cell::RefCell<Vec<String>>,
    }

    /// Un `Rc` est aussi un serveur : c'est ce qui permet au test de garder un
    /// handle sur le faux pendant que `App` en possède un exemplaire, et donc
    /// de relire ce qui lui a été demandé.
    impl Herdr for std::rc::Rc<FakeHerdr> {
        fn agent_list(&self) -> Option<String> {
            (**self).agent_list()
        }
        fn tab_list(&self) -> Option<String> {
            (**self).tab_list()
        }
        fn send_input(&self, pane_id: &str, text: &str) -> Result<(), String> {
            (**self).send_input(pane_id, text)
        }
        fn focus_agent(&self, pane_id: &str) {
            (**self).focus_agent(pane_id)
        }
    }

    fn boxed(fake: FakeHerdr) -> Box<dyn Herdr> {
        Box::new(std::rc::Rc::new(fake))
    }

    impl Herdr for FakeHerdr {
        fn agent_list(&self) -> Option<String> {
            Some(self.agents.clone())
        }
        fn tab_list(&self) -> Option<String> {
            Some(self.tabs.clone())
        }
        fn send_input(&self, pane_id: &str, text: &str) -> Result<(), String> {
            if let Some(e) = &self.refuse {
                return Err(e.clone());
            }
            self.sent
                .borrow_mut()
                .push((pane_id.to_owned(), text.to_owned()));
            Ok(())
        }
        fn focus_agent(&self, pane_id: &str) {
            self.focused.borrow_mut().push(pane_id.to_owned());
        }
    }

    fn agents_json(entries: &[(&str, &str, &str)]) -> String {
        let list: Vec<serde_json::Value> = entries
            .iter()
            .map(|(pane_id, agent, tab_id)| {
                serde_json::json!({
                    "pane_id": pane_id, "agent": agent, "tab_id": tab_id,
                })
            })
            .collect();
        serde_json::json!({ "result": { "agents": list } }).to_string()
    }

    fn tabs_json(ids: &[&str]) -> String {
        let list: Vec<serde_json::Value> = ids
            .iter()
            .map(|id| serde_json::json!({ "tab_id": id }))
            .collect();
        serde_json::json!({ "result": { "tabs": list } }).to_string()
    }

    /// Deux agents dans **la même tab**, `w1:p1` (claude) et `w1:p2` (codex) :
    /// ils partagent donc ce buffer et ne changent que la destination.
    fn two_agents(refuse: Option<&str>) -> FakeHerdr {
        FakeHerdr {
            agents: agents_json(&[
                ("w1:p1", "claude", TEST_TAB),
                ("w1:p2", "codex", TEST_TAB),
            ]),
            tabs: tabs_json(&[TEST_TAB]),
            refuse: refuse.map(str::to_owned),
            ..Default::default()
        }
    }

    fn wired(text: &str, refuse: Option<&str>) -> App {
        wired_on(text, std::rc::Rc::new(two_agents(refuse)))
    }

    /// Idem, mais l'appelant garde le faux serveur pour relire sa trace.
    fn wired_on(text: &str, herdr: std::rc::Rc<FakeHerdr>) -> App {
        let mut app = App::with_herdr(text, Box::new(herdr));
        app.refresh_targets();
        app
    }

    /// Un serveur sans le moindre agent.
    fn wired_empty(text: &str) -> App {
        let mut app = App::with_herdr(
            text,
            boxed(FakeHerdr {
                agents: agents_json(&[]),
                tabs: tabs_json(&[TEST_TAB]),
                ..Default::default()
            }),
        );
        app.refresh_targets();
        app
    }

    fn status_of(app: &App) -> String {
        app.status.as_ref().map(|(m, _)| m.clone()).unwrap_or_default()
    }

    #[test]
    fn typing_inserts_text() {
        let mut app = App::headless("");
        app.on_key(key(KeyCode::Char('a')));
        app.on_key(key(KeyCode::Char('b')));
        assert_eq!(text_of(&app), "ab");
        assert!(app.dirty);
    }

    #[test]
    fn key_release_is_ignored() {
        let mut app = App::headless("");
        let mut k = key(KeyCode::Char('a'));
        k.kind = KeyEventKind::Release;
        app.on_key(k);
        assert_eq!(text_of(&app), "");
    }

    /// AltGr sur Windows arrive en CONTROL|ALT : `@` doit s'écrire, pas
    /// déclencher une commande.
    #[test]
    fn altgr_types_a_character_instead_of_running_a_command() {
        let mut app = App::headless("");
        app.on_key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        ));
        assert_eq!(text_of(&app), "c");
    }

    #[test]
    fn clear_empties_and_stashes() {
        let mut app = App::headless("du texte");
        app.on_key(ctrl('l'));
        assert_eq!(text_of(&app), "");
        assert_eq!(app.stash.as_deref(), Some("du texte"));
    }

    #[test]
    fn clear_needs_no_confirmation() {
        let mut app = App::headless("du texte");
        app.on_key(ctrl('l'));
        // Aucune touche supplémentaire : le vidage a déjà eu lieu.
        assert_eq!(text_of(&app), "");
    }

    #[test]
    fn undo_restores_the_cleared_text() {
        let mut app = App::headless("du texte");
        app.on_key(ctrl('l'));
        app.on_key(ctrl('z'));
        assert_eq!(text_of(&app), "du texte");
    }

    #[test]
    fn undo_without_a_stash_says_so_and_changes_nothing() {
        let mut app = App::headless("intact");
        app.on_key(ctrl('z'));
        assert_eq!(text_of(&app), "intact");
        assert!(app.status.as_ref().unwrap().0.contains("nothing"));
    }

    /// La case n'a qu'une place : un second vidage remplace le contenu
    /// rattrapable, il ne l'empile pas.
    #[test]
    fn the_stash_holds_only_the_last_clear() {
        let mut app = App::headless("premier");
        app.on_key(ctrl('l'));
        app.on_paste("second");
        app.on_key(ctrl('l'));
        app.on_key(ctrl('z'));
        assert_eq!(text_of(&app), "second");
    }

    #[test]
    fn undo_is_not_an_edit_undo() {
        let mut app = App::headless("");
        app.on_key(key(KeyCode::Char('a')));
        app.on_key(ctrl('z'));
        assert_eq!(text_of(&app), "a", "Ctrl+Z ne défait pas la frappe");
    }

    #[test]
    fn clearing_an_empty_buffer_does_not_destroy_the_stash() {
        let mut app = App::headless("récupérable");
        app.on_key(ctrl('l'));
        app.on_key(ctrl('l')); // déjà vide
        app.on_key(ctrl('z'));
        assert_eq!(text_of(&app), "récupérable");
    }

    #[test]
    fn paste_arrives_as_one_block() {
        let mut app = App::headless("");
        app.on_paste("une\nligne collée");
        assert_eq!(text_of(&app), "une\nligne collée");
    }

    #[test]
    fn copy_of_an_empty_buffer_reports_nothing_to_copy() {
        let mut app = App::headless("");
        app.on_key(ctrl('c'));
        assert!(app.status.as_ref().unwrap().0.contains("nothing to copy"));
    }

    #[test]
    fn copy_beyond_the_limit_points_at_the_export() {
        let mut app = App::headless(&"x".repeat(MAX_CLIPBOARD_BYTES + 1));
        app.on_key(ctrl('c'));
        let message = &app.status.as_ref().unwrap().0;
        assert!(message.contains("^S"), "message : {message}");
    }

    #[test]
    fn shift_click_is_left_to_the_terminal() {
        let mut app = App::headless("du texte");
        app.buttons = vec![(Action::Command(Command::Clear), Rect { x: 0, y: 0, width: 8, height: 1 })];
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 1,
            row: 0,
            modifiers: KeyModifiers::SHIFT,
        });
        assert_eq!(text_of(&app), "du texte", "Shift+clic ne doit rien déclencher");
    }

    #[test]
    fn clicking_a_button_runs_its_command() {
        let mut app = App::headless("du texte");
        app.buttons = vec![(Action::Command(Command::Clear), Rect { x: 0, y: 0, width: 8, height: 1 })];
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 1,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(text_of(&app), "");
    }

    #[test]
    fn clicking_beside_a_button_does_nothing() {
        let mut app = App::headless("du texte");
        app.buttons = vec![(Action::Command(Command::Clear), Rect { x: 0, y: 0, width: 8, height: 1 })];
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 40,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(text_of(&app), "du texte");
    }

    #[test]
    fn clicking_in_the_text_places_the_cursor() {
        let mut app = App::headless("un deux");
        app.body = Rect { x: 0, y: 0, width: 10, height: 3 };
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 3,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        app.on_key(key(KeyCode::Char('X')));
        assert_eq!(text_of(&app), "un Xdeux", "la frappe suit le clic");
    }

    #[test]
    fn clicking_a_button_does_not_move_the_cursor() {
        let mut app = App::headless("texte");
        app.body = Rect { x: 0, y: 0, width: 20, height: 3 };
        app.buttons = vec![(Action::CycleTarget, Rect { x: 0, y: 0, width: 8, height: 1 })];
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 1,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        app.on_key(key(KeyCode::Char('X')));
        assert_eq!(
            text_of(&app),
            "texteX",
            "la barre prime sur le texte qu'elle recouvre"
        );
    }

    #[test]
    fn ctrl_arrow_jumps_a_word() {
        let mut app = App::headless("un deux");
        app.on_key(ctrl_code(KeyCode::Left));
        app.on_key(key(KeyCode::Char('X')));
        assert_eq!(text_of(&app), "un Xdeux");
    }

    #[test]
    fn ctrl_home_and_end_jump_to_the_ends_of_the_buffer() {
        let mut app = App::headless("un\ndeux\ntrois");
        app.on_key(ctrl_code(KeyCode::Home));
        app.on_key(key(KeyCode::Char('X')));
        app.on_key(ctrl_code(KeyCode::End));
        app.on_key(key(KeyCode::Char('Y')));
        assert_eq!(text_of(&app), "Xun\ndeux\ntroisY");
    }

    #[test]
    fn ctrl_backspace_deletes_the_word_on_the_left() {
        let mut app = App::headless("un deux");
        app.on_key(ctrl_code(KeyCode::Backspace));
        assert_eq!(text_of(&app), "un ");
    }

    #[test]
    fn ctrl_h_is_the_same_as_ctrl_backspace() {
        let mut app = App::headless("un deux");
        app.on_key(ctrl('h'));
        assert_eq!(
            text_of(&app),
            "un ",
            "certains terminaux encodent Ctrl+Backspace en ^H"
        );
    }

    #[test]
    fn an_unknown_ctrl_combination_does_not_type_its_letter() {
        let mut app = App::headless("");
        app.on_key(ctrl('d'));
        assert_eq!(text_of(&app), "", "Ctrl+D n'écrit pas un « d »");
    }

    #[test]
    fn wheel_scrolls_without_touching_the_text() {
        let mut app = App::headless("a\nb\nc\nd\ne\nf\ng\nh");
        app.total_rows = 8;
        app.body = Rect { x: 0, y: 0, width: 10, height: 2 };
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.scroll, WHEEL_STEP);
        assert_eq!(text_of(&app), "a\nb\nc\nd\ne\nf\ng\nh");
    }

    #[test]
    fn wheel_does_not_scroll_past_the_end() {
        let mut app = App::headless("a\nb");
        app.total_rows = 2;
        app.body = Rect { x: 0, y: 0, width: 10, height: 2 };
        for _ in 0..10 {
            app.on_mouse(MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: 0,
                row: 0,
                modifiers: KeyModifiers::NONE,
            });
        }
        assert_eq!(app.scroll, 0);
    }

    #[test]
    fn there_is_no_quit_key() {
        let mut app = App::headless("");
        for c in ['q', 'x', 'd', 'w'] {
            app.on_key(ctrl(c));
            app.on_key(key(KeyCode::Char(c)));
        }
        app.on_key(key(KeyCode::Esc));
        // Rien à assurer sinon que rien n'a paniqué et que le texte a reçu les
        // caractères ordinaires : aucune commande ne quitte.
        assert_eq!(text_of(&app), "qxdw");
    }

    #[test]
    fn human_sizes_are_readable() {
        assert_eq!(human(0), "0 B");
        assert_eq!(human(512), "512 B");
        assert_eq!(human(1024), "1.0 KB");
        assert_eq!(human(1024 * 1024), "1.0 MB");
    }
    // -- envoyer à l'agent -------------------------------------------------

    #[test]
    fn sending_an_empty_buffer_clears_nothing_and_says_so() {
        let mut app = wired("", None);
        app.on_key(ctrl('e'));
        assert!(status_of(&app).contains("nothing to emit"), "{}", status_of(&app));
        assert!(app.stash.is_none(), "la case de secours doit rester intacte");
    }

    #[test]
    fn a_successful_send_drops_the_text_on_the_displayed_target() {
        let mut app = wired("à envoyer", None);
        app.on_key(ctrl('e'));
        assert_eq!(text_of(&app), "");
        assert!(status_of(&app).starts_with("emitted → claude"), "{}", status_of(&app));
    }

    /// Le dépôt est un **déplacement** : `Ctrl+Z` doit le rattraper, comme un
    /// vidage.
    #[test]
    fn a_successful_send_clears_and_undo_restores() {
        let mut app = wired("à envoyer", None);
        app.on_key(ctrl('e'));
        app.on_key(ctrl('z'));
        assert_eq!(text_of(&app), "à envoyer");
    }

    /// C'est ce qui rend l'erreur sans conséquence, et ce qui remplace la
    /// confirmation.
    #[test]
    fn a_failed_send_does_not_clear() {
        let mut app = wired("précieux", Some("pane_not_found"));
        app.on_key(ctrl('e'));
        assert_eq!(text_of(&app), "précieux");
        assert!(app.stash.is_none());
        assert!(status_of(&app).contains("failed"), "{}", status_of(&app));
    }

    #[test]
    fn without_an_agent_the_send_refuses_and_does_not_clear() {
        let mut app = wired_empty("précieux");
        app.on_key(ctrl('e'));
        assert_eq!(text_of(&app), "précieux");
        assert!(status_of(&app).contains("no agent"), "{}", status_of(&app));
    }

    /// La cible affichée a disparu entre l'affichage et l'appui : on ne se
    /// rabat **pas** sur une autre, on refuse.
    #[test]
    fn a_vanished_target_cancels_the_send_instead_of_picking_another() {
        let mut app = wired("précieux", None);
        assert_eq!(app.current_target().unwrap().pane_id, "w1:p1");

        // L'agent visé ferme son pane ; un autre reste joignable.
        app.herdr = boxed(FakeHerdr {
            agents: agents_json(&[("w1:p2", "codex", TEST_TAB)]),
            tabs: tabs_json(&[TEST_TAB]),
            ..Default::default()
        });
        app.on_key(ctrl('e'));

        assert_eq!(text_of(&app), "précieux");
        assert!(status_of(&app).contains("not found"), "{}", status_of(&app));
    }

    /// Le focus va au pane qui a **effectivement** reçu le texte — le même que
    /// celui du dépôt, pas la cible d'avant le cyclage.
    #[test]
    fn a_successful_send_focuses_the_agent_that_got_the_text() {
        let herdr = std::rc::Rc::new(two_agents(None));
        let mut app = wired_on("à envoyer", herdr.clone());
        app.on_key(ctrl('n'));
        let visee = app.current_target().unwrap().pane_id.clone();
        app.on_key(ctrl('e'));

        assert_eq!(
            herdr.sent.borrow().clone(),
            vec![(visee.clone(), "à envoyer".to_owned())]
        );
        assert_eq!(herdr.focused.borrow().clone(), vec![visee]);
    }

    /// Rien n'est parti : on ne bascule pas, et le texte reste sous les yeux.
    #[test]
    fn a_failed_send_focuses_nothing() {
        let herdr = std::rc::Rc::new(two_agents(Some("pane_not_found")));
        let mut app = wired_on("précieux", herdr.clone());
        app.on_key(ctrl('e'));
        assert!(herdr.focused.borrow().is_empty());
        assert_eq!(text_of(&app), "précieux");
    }

    #[test]
    fn an_empty_buffer_focuses_nothing() {
        let herdr = std::rc::Rc::new(two_agents(None));
        let mut app = wired_on("", herdr.clone());
        app.on_key(ctrl('e'));
        assert!(herdr.focused.borrow().is_empty());
    }

    #[test]
    fn cycling_changes_the_target_without_touching_the_text() {
        let mut app = wired("intact", None);
        assert_eq!(app.current_target().unwrap().agent, "claude");
        app.on_key(ctrl('n'));
        assert_eq!(app.current_target().unwrap().agent, "codex");
        app.on_key(ctrl('n'));
        assert_eq!(app.current_target().unwrap().agent, "claude", "le cyclage boucle");
        assert_eq!(text_of(&app), "intact");
    }

    /// Un message remplace les boutons pendant trois secondes : en afficher un
    /// au cyclage masquerait la zone cible, et il faudrait attendre pour
    /// recliquer dessus.
    #[test]
    fn cycling_does_not_hide_the_bar() {
        let mut app = wired("x", None);
        app.on_key(ctrl('n'));
        assert!(app.status.is_none(), "le cyclage ne doit rien afficher");
    }

    /// Même sans cible, le cyclage reste muet : la zone dit déjà « aucun
    /// agent », et elle doit rester cliquable.
    #[test]
    fn cycling_without_an_agent_stays_silent() {
        let mut app = wired_empty("x");
        app.on_key(ctrl('n'));
        assert!(app.status.is_none());
        assert!(app.current_target().is_none());
    }

    /// Deux clics de suite sur la zone cible doivent avancer de deux crans.
    #[test]
    fn two_clicks_on_the_target_area_advance_twice() {
        let mut app = wired("x", None);
        app.buttons = vec![(Action::CycleTarget, Rect { x: 0, y: 0, width: 12, height: 1 })];
        let depart = app.current_target().unwrap().pane_id.clone();

        for _ in 0..2 {
            app.on_mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 1,
                row: 0,
                modifiers: KeyModifiers::NONE,
            });
        }
        // Deux cibles en tout : deux crans ramènent au départ.
        assert_eq!(app.current_target().unwrap().pane_id, depart);
        assert!(app.status.is_none());
    }

    #[test]
    fn the_cycled_target_survives_a_refresh() {
        let mut app = wired("x", None);
        app.on_key(ctrl('n'));
        app.refresh_targets();
        assert_eq!(app.current_target().unwrap().agent, "codex");
    }

    /// Un `agent.list` muet ne doit pas faire disparaître le bouton d'envoi :
    /// mieux vaut une liste un peu vieille qu'une barre clignotante.
    #[test]
    fn a_silent_server_leaves_the_previous_list_in_place() {
        struct Muet;
        impl Herdr for Muet {
            fn agent_list(&self) -> Option<String> {
                None
            }
            fn tab_list(&self) -> Option<String> {
                None
            }
            fn send_input(&self, _: &str, _: &str) -> Result<(), String> {
                Err("injoignable".into())
            }
            fn focus_agent(&self, _: &str) {}
        }
        let mut app = wired("x", None);
        app.herdr = Box::new(Muet);
        app.refresh_targets();
        assert_eq!(app.current_target().unwrap().agent, "claude");
    }

    #[test]
    fn clicking_the_target_area_cycles_and_clicking_send_sends() {
        let mut app = wired("à envoyer", None);
        app.buttons = vec![
            (Action::Command(Command::Send), Rect { x: 0, y: 0, width: 10, height: 1 }),
            (Action::CycleTarget, Rect { x: 11, y: 0, width: 12, height: 1 }),
        ];

        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 12,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.current_target().unwrap().agent, "codex");
        assert_eq!(text_of(&app), "à envoyer", "cycler ne touche pas au texte");

        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 3,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(text_of(&app), "");
        assert!(status_of(&app).contains("codex"), "{}", status_of(&app));
    }

    #[test]
    fn shift_click_on_the_send_bar_stays_inert() {
        let mut app = wired("intact", None);
        app.buttons = vec![
            (Action::Command(Command::Send), Rect { x: 0, y: 0, width: 10, height: 1 }),
        ];
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 1,
            row: 0,
            modifiers: KeyModifiers::SHIFT,
        });
        assert_eq!(text_of(&app), "intact");
    }

    #[test]
    fn the_current_pane_never_offers_itself_as_a_target() {
        let mut app = App::with_herdr(
            "x",
            boxed(FakeHerdr {
                agents: agents_json(&[("w1:p9", "claude", TEST_TAB)]),
                tabs: tabs_json(&[TEST_TAB]),
                ..Default::default()
            }),
        );
        app.pane_id = Some("w1:p9".into());
        app.refresh_targets();
        assert!(app.current_target().is_none());
    }

    /// La cible par défaut est la première de ma tab, par `pane_id`. L'ordre
    /// étant stable, « la première » désigne toujours le même agent.
    #[test]
    fn the_default_target_is_the_first_agent_in_my_tab() {
        let app = wired("x", None);
        assert_eq!(app.current_target().unwrap().pane_id, "w1:p1");
    }

    /// Le cloisonnement, vu de l'application : un agent qui vit ailleurs
    /// n'est pas joignable, et le bouton d'envoi n'existe donc pas.
    #[test]
    fn an_agent_from_another_tab_is_never_a_target() {
        let mut app = App::with_herdr(
            "précieux",
            boxed(FakeHerdr {
                agents: agents_json(&[("w9:p1", "claude", "w9:t9")]),
                tabs: tabs_json(&[TEST_TAB, "w9:t9"]),
                ..Default::default()
            }),
        );
        app.refresh_targets();

        assert!(app.targets.is_empty(), "l'agent d'à côté n'est pas chez moi");
        app.on_key(ctrl('e'));
        assert_eq!(text_of(&app), "précieux", "rien ne part, rien ne se vide");
    }

    /// Un seul agent : nulle part où aller, donc rien ne bouge et rien ne
    /// s'affiche — la zone cible n'existe même pas dans ce cas.
    #[test]
    fn cycling_does_nothing_and_says_nothing_with_a_single_agent() {
        let mut app = App::with_herdr(
            "x",
            boxed(FakeHerdr {
                agents: agents_json(&[("w1:p1", "claude", TEST_TAB)]),
                tabs: tabs_json(&[TEST_TAB]),
                ..Default::default()
            }),
        );
        app.refresh_targets();

        app.on_key(ctrl('n'));
        assert_eq!(app.current_target().unwrap().pane_id, "w1:p1");
        assert!(app.status.is_none(), "le cyclage n'a jamais de retour");
    }

    // -- ménage des buffers orphelins --------------------------------------

    #[test]
    fn the_sweep_keeps_the_live_tabs() {
        let herdr = std::rc::Rc::new(two_agents(None));
        assert_eq!(live_tabs(&herdr), Some(vec![TEST_TAB.to_owned()]));
    }

    /// Une liste vide n'est jamais une information : c'est une panne, et le
    /// ménage s'abstient plutôt que de déclarer toutes les tabs mortes.
    #[test]
    fn the_sweep_abstains_when_tab_list_returns_an_empty_list() {
        let herdr = std::rc::Rc::new(FakeHerdr {
            tabs: tabs_json(&[]),
            ..Default::default()
        });
        assert_eq!(live_tabs(&herdr), None);
    }

    #[test]
    fn the_sweep_abstains_when_the_server_is_silent() {
        struct Muet;
        impl Herdr for Muet {
            fn agent_list(&self) -> Option<String> {
                None
            }
            fn tab_list(&self) -> Option<String> {
                None
            }
            fn send_input(&self, _: &str, _: &str) -> Result<(), String> {
                Err("injoignable".into())
            }
            fn focus_agent(&self, _: &str) {}
        }
        assert_eq!(live_tabs(&Muet), None);
    }

    #[test]
    fn the_sweep_abstains_on_unreadable_json() {
        let herdr = std::rc::Rc::new(FakeHerdr {
            tabs: "pas du json".into(),
            ..Default::default()
        });
        assert_eq!(live_tabs(&herdr), None);
    }

    /// AltGr ne doit pas non plus déclencher les nouvelles commandes.
    #[test]
    fn altgr_does_not_send() {
        let mut app = wired("intact", None);
        app.on_key(KeyEvent::new(
            KeyCode::Char('e'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        ));
        assert_eq!(text_of(&app), "intacte");
    }
}
