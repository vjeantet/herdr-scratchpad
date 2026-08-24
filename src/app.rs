//! L'état du pane et sa machine à événements.

use std::time::{Duration, Instant};

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use ratatui::Frame;

use crate::agents::{self, Home, Target};
use crate::buffer::{Buffer, PAGE_FALLBACK};
use crate::clipboard::{self, CopyError, MAX_CLIPBOARD_BYTES};
use crate::ipc::{Herdr, Socket};
use crate::state::{self, Store};
use crate::ui;

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
/// secondes et demie suffisent donc largement, pour deux appels socket.
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
    /// Où vit ce pane : sa tab d'abord, son workspace ensuite. C'est l'ordre
    /// de préférence de la cible par défaut.
    home: Home,
    /// Agents joignables, rafraîchis toutes les [`TARGET_REFRESH`].
    targets: Vec<Target>,
    /// Index de la cible dans `targets`.
    target: Option<usize>,
    /// Dernière cible retenue sur disque : *(libellé de workspace, agent)*.
    remembered: Option<(String, String)>,
}

impl App {
    pub fn new() -> Self {
        let mut store = Store::from_env();
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
            pane_id: std::env::var("HERDR_PANE_ID").ok().filter(|s| !s.is_empty()),
            herdr: Box::new(Socket),
            home: Home {
                tab_id: env("HERDR_TAB_ID"),
                workspace_id: env("HERDR_WORKSPACE_ID"),
            },
            targets: Vec::new(),
            target: None,
            remembered: state::load_target(),
        };
        app.stamp();
        // La barre doit afficher une cible dès le premier rendu : sans ça elle
        // annoncerait « aucun agent » pendant les deux premières secondes et
        // demie, ce qui se lit comme une panne.
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
            home: Home::default(),
            targets: Vec::new(),
            target: None,
            remembered: None,
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
        let target = self.target.and_then(|i| self.targets.get(i));

        let geom = ui::draw(
            frame,
            self.buf.lines(),
            self.buf.cursor(),
            &mut self.scroll,
            status,
            self.buf.is_empty(),
            target,
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
                if let Some((action, _)) = self.buttons.iter().find(|(_, r)| r.contains(pos)) {
                    match *action {
                        Action::Command(command) => self.run(command),
                        Action::CycleTarget => self.cycle_target(),
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
            Ok(len) => self.say(format!("copié · {}", human(len))),
            Err(CopyError::Empty) => self.say("rien à copier".into()),
            Err(CopyError::TooLarge { len }) => self.say(format!(
                "{} > {} — ^S écrit le fichier",
                human(len),
                human(MAX_CLIPBOARD_BYTES)
            )),
            Err(CopyError::Io(e)) => self.say(format!("copie impossible : {e}")),
        }
    }

    fn clear(&mut self) {
        let text = self.buf.text();
        if text.is_empty() {
            self.say("déjà vide".into());
            return;
        }
        self.stash_and_clear(text);
        self.say("vidé · ^Z annule".into());
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
                self.say("restauré".into());
            }
            None => self.say("rien à restaurer".into()),
        }
    }

    fn export(&mut self) {
        let text = self.buf.text();
        match state::export(&text) {
            Ok(path) => self.say(path.display().to_string()),
            Err(e) => self.say(format!("export impossible : {e}")),
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
            self.say("rien à envoyer".into());
            return;
        }

        // Ce que la barre affichait au moment du clic. C'est **cette** cible
        // qu'on envoie, pas celle qu'un rafraîchissement choisirait à sa
        // place : le garde-fou est l'affichage, il perdrait tout son sens si
        // la destination pouvait changer entre la lecture et l'appui.
        let Some(intended) = self.current_target().map(|t| t.pane_id.clone()) else {
            self.say("aucun agent".into());
            return;
        };

        // Re-résolution juste avant d'agir : l'affichage n'a besoin d'être
        // qu'à peu près à jour, l'action doit être exactement juste.
        self.refresh_targets();
        let Some(target) = self.targets.iter().find(|t| t.pane_id == intended).cloned() else {
            self.say("agent introuvable".into());
            return;
        };

        if let Err(e) = self.herdr.send_input(&target.pane_id, &text) {
            self.say(format!("envoi impossible : {e}"));
            return;
        }

        state::save_target(&target.workspace_label, &target.agent);
        self.remembered = Some((target.workspace_label.clone(), target.agent.clone()));
        self.stash_and_clear(text);
        self.say(format!(
            "envoyé → {}·{} · ^Z annule",
            target.agent, target.workspace_label
        ));

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
    fn cycle_target(&mut self) {
        self.target = agents::next(&self.targets, self.target);
        match self.current_target() {
            Some(t) => self.say(format!("→ {}·{}", t.agent, t.workspace_label)),
            None => self.say("aucun agent".into()),
        }
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
            self.status = Some((format!("sauvegarde impossible : {e}"), Instant::now()));
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
    /// qui hoquette ne doit pas afficher « aucun agent » une demi-seconde.
    fn refresh_targets(&mut self) {
        let (Some(agents_json), Some(workspaces_json)) =
            (self.herdr.agent_list(), self.herdr.workspace_list())
        else {
            return;
        };
        let kept = self.current_target().map(|t| t.pane_id.clone());
        self.targets = agents::targets(
            &agents_json,
            &workspaces_json,
            self.pane_id.as_deref(),
        );
        self.target = kept
            .and_then(|id| self.targets.iter().position(|t| t.pane_id == id))
            .or_else(|| {
                agents::pick_default(
                    &self.targets,
                    &self.home,
                    self.remembered
                        .as_ref()
                        .map(|(label, agent)| (label.as_str(), agent.as_str())),
                )
            });
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

/// Taille lisible. Les tailles qui comptent ici vont de l'octet au méga-octet.
fn human(bytes: usize) -> String {
    const KO: usize = 1024;
    const MO: usize = KO * KO;
    if bytes >= MO {
        format!("{:.1} Mo", bytes as f64 / MO as f64)
    } else if bytes >= KO {
        format!("{:.1} Ko", bytes as f64 / KO as f64)
    } else {
        format!("{bytes} o")
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

    fn text_of(app: &App) -> String {
        app.buf.text()
    }

    /// Un serveur herdr en carton : des réponses figées, et la trace de ce
    /// qui lui a été déposé. Aucun test unitaire n'ouvre de socket.
    #[derive(Default)]
    struct FakeHerdr {
        agents: String,
        workspaces: String,
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
        fn workspace_list(&self) -> Option<String> {
            (**self).workspace_list()
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
        fn workspace_list(&self) -> Option<String> {
            Some(self.workspaces.clone())
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

    fn agents_json(entries: &[(&str, &str, &str, &str)]) -> String {
        let list: Vec<serde_json::Value> = entries
            .iter()
            .map(|(pane_id, agent, tab_id, workspace_id)| {
                serde_json::json!({
                    "pane_id": pane_id, "agent": agent,
                    "tab_id": tab_id, "workspace_id": workspace_id,
                })
            })
            .collect();
        serde_json::json!({ "result": { "agents": list } }).to_string()
    }

    fn workspaces_json(entries: &[(&str, &str)]) -> String {
        let list: Vec<serde_json::Value> = entries
            .iter()
            .map(|(id, label)| serde_json::json!({ "workspace_id": id, "label": label }))
            .collect();
        serde_json::json!({ "result": { "workspaces": list } }).to_string()
    }

    /// Deux agents joignables, `w1:p1` (claude·un) et `w2:p1` (codex·deux).
    fn two_agents(refuse: Option<&str>) -> FakeHerdr {
        FakeHerdr {
            agents: agents_json(&[
                ("w1:p1", "claude", "w1:t1", "w1"),
                ("w2:p1", "codex", "w2:t1", "w2"),
            ]),
            workspaces: workspaces_json(&[("w1", "un"), ("w2", "deux")]),
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
                workspaces: workspaces_json(&[]),
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
        assert!(app.status.as_ref().unwrap().0.contains("rien"));
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
        assert!(app.status.as_ref().unwrap().0.contains("rien à copier"));
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
        assert_eq!(human(0), "0 o");
        assert_eq!(human(512), "512 o");
        assert_eq!(human(1024), "1.0 Ko");
        assert_eq!(human(1024 * 1024), "1.0 Mo");
    }
    // -- envoyer à l'agent -------------------------------------------------

    #[test]
    fn ctrl_e_sur_un_buffer_vide_ne_vide_rien_et_le_dit() {
        let mut app = wired("", None);
        app.on_key(ctrl('e'));
        assert!(status_of(&app).contains("rien à envoyer"), "{}", status_of(&app));
        assert!(app.stash.is_none(), "la case de secours doit rester intacte");
    }

    #[test]
    fn un_envoi_reussi_depose_le_texte_chez_la_cible_affichee() {
        let mut app = wired("à envoyer", None);
        app.on_key(ctrl('e'));
        assert_eq!(text_of(&app), "");
        assert!(status_of(&app).starts_with("envoyé → claude·un"), "{}", status_of(&app));
    }

    /// Le dépôt est un **déplacement** : `Ctrl+Z` doit le rattraper, comme un
    /// vidage.
    #[test]
    fn un_envoi_reussi_vide_et_ctrl_z_restaure() {
        let mut app = wired("à envoyer", None);
        app.on_key(ctrl('e'));
        app.on_key(ctrl('z'));
        assert_eq!(text_of(&app), "à envoyer");
    }

    /// C'est ce qui rend l'erreur sans conséquence, et ce qui remplace la
    /// confirmation.
    #[test]
    fn un_envoi_qui_echoue_ne_vide_pas() {
        let mut app = wired("précieux", Some("pane_not_found"));
        app.on_key(ctrl('e'));
        assert_eq!(text_of(&app), "précieux");
        assert!(app.stash.is_none());
        assert!(status_of(&app).contains("impossible"), "{}", status_of(&app));
    }

    #[test]
    fn sans_agent_l_envoi_refuse_et_ne_vide_pas() {
        let mut app = wired_empty("précieux");
        app.on_key(ctrl('e'));
        assert_eq!(text_of(&app), "précieux");
        assert!(status_of(&app).contains("aucun agent"), "{}", status_of(&app));
    }

    /// La cible affichée a disparu entre l'affichage et l'appui : on ne se
    /// rabat **pas** sur une autre, on refuse.
    #[test]
    fn une_cible_disparue_annule_l_envoi_au_lieu_d_en_choisir_une_autre() {
        let mut app = wired("précieux", None);
        assert_eq!(app.current_target().unwrap().pane_id, "w1:p1");

        // L'agent visé ferme son pane ; un autre reste joignable.
        app.herdr = boxed(FakeHerdr {
            agents: agents_json(&[("w2:p1", "codex", "w2:t1", "w2")]),
            workspaces: workspaces_json(&[("w2", "deux")]),
            ..Default::default()
        });
        app.on_key(ctrl('e'));

        assert_eq!(text_of(&app), "précieux");
        assert!(status_of(&app).contains("introuvable"), "{}", status_of(&app));
    }

    /// Le focus va au pane qui a **effectivement** reçu le texte — le même que
    /// celui du dépôt, pas la cible d'avant le cyclage.
    #[test]
    fn un_envoi_reussi_focalise_l_agent_qui_a_recu_le_texte() {
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
    fn un_envoi_qui_echoue_ne_focalise_rien() {
        let herdr = std::rc::Rc::new(two_agents(Some("pane_not_found")));
        let mut app = wired_on("précieux", herdr.clone());
        app.on_key(ctrl('e'));
        assert!(herdr.focused.borrow().is_empty());
        assert_eq!(text_of(&app), "précieux");
    }

    #[test]
    fn un_buffer_vide_ne_focalise_rien() {
        let herdr = std::rc::Rc::new(two_agents(None));
        let mut app = wired_on("", herdr.clone());
        app.on_key(ctrl('e'));
        assert!(herdr.focused.borrow().is_empty());
    }

    #[test]
    fn ctrl_n_fait_tourner_la_cible_sans_toucher_au_texte() {
        let mut app = wired("intact", None);
        assert_eq!(app.current_target().unwrap().agent, "claude");
        app.on_key(ctrl('n'));
        assert_eq!(app.current_target().unwrap().agent, "codex");
        app.on_key(ctrl('n'));
        assert_eq!(app.current_target().unwrap().agent, "claude", "le cyclage boucle");
        assert_eq!(text_of(&app), "intact");
    }

    #[test]
    fn le_cyclage_survit_a_un_rafraichissement() {
        let mut app = wired("x", None);
        app.on_key(ctrl('n'));
        app.refresh_targets();
        assert_eq!(app.current_target().unwrap().agent, "codex");
    }

    /// Un `agent.list` muet ne doit pas faire disparaître la barre : mieux
    /// vaut une liste un peu vieille qu'un « aucun agent » clignotant.
    #[test]
    fn un_serveur_muet_laisse_la_liste_precedente_en_place() {
        struct Muet;
        impl Herdr for Muet {
            fn agent_list(&self) -> Option<String> {
                None
            }
            fn workspace_list(&self) -> Option<String> {
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
    fn un_clic_sur_la_zone_cible_cycle_et_un_clic_sur_envoyer_envoie() {
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
        assert!(status_of(&app).contains("codex·deux"), "{}", status_of(&app));
    }

    #[test]
    fn shift_clic_sur_la_barre_d_envoi_reste_inerte() {
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
    fn le_pane_courant_ne_se_propose_jamais_comme_cible() {
        let mut app = App::with_herdr(
            "x",
            boxed(FakeHerdr {
                agents: agents_json(&[("w1:p9", "claude", "w1:t1", "w1")]),
                workspaces: workspaces_json(&[("w1", "un")]),
                ..Default::default()
            }),
        );
        app.pane_id = Some("w1:p9".into());
        app.refresh_targets();
        assert!(app.current_target().is_none());
    }

    /// Première préférence : l'agent de la tab de ce pane — « l'agent du pane
    /// courant », celui d'où le scratchpad a été ouvert.
    #[test]
    fn la_cible_par_defaut_est_l_agent_de_la_tab_courante() {
        let herdr = FakeHerdr {
            // Deux agents dans le MÊME workspace, dans deux tabs : c'est le
            // cas où la préférence par workspace se trompait de voisin.
            agents: agents_json(&[
                ("w3:p1", "claude", "w3:t1", "w3"),
                ("w3:p2", "claude", "w3:t2", "w3"),
            ]),
            workspaces: workspaces_json(&[("w3", "trois")]),
            ..Default::default()
        };
        let mut app = App::with_herdr("x", boxed(herdr));
        app.home = Home {
            tab_id: Some("w3:t2".into()),
            workspace_id: Some("w3".into()),
        };
        app.refresh_targets();
        assert_eq!(app.current_target().unwrap().pane_id, "w3:p2");
    }

    /// À défaut d'agent dans la tab, on retombe sur le workspace.
    #[test]
    fn sans_agent_dans_la_tab_la_cible_par_defaut_reste_le_workspace() {
        let mut app = wired("x", None);
        app.home = Home {
            tab_id: Some("w2:t9".into()),
            workspace_id: Some("w2".into()),
        };
        app.target = None;
        app.refresh_targets();
        assert_eq!(app.current_target().unwrap().agent, "codex");
    }

    /// AltGr ne doit pas non plus déclencher les nouvelles commandes.
    #[test]
    fn altgr_n_envoie_pas() {
        let mut app = wired("intact", None);
        app.on_key(KeyEvent::new(
            KeyCode::Char('e'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        ));
        assert_eq!(text_of(&app), "intacte");
    }
}
