//! L'état du pane et sa machine à événements.

use std::time::{Duration, Instant};

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use ratatui::Frame;

use crate::buffer::{Buffer, PAGE_FALLBACK};
use crate::clipboard::{self, CopyError, MAX_CLIPBOARD_BYTES};
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

/// Les quatre commandes. Rien d'autre — il n'y a pas de touche pour quitter :
/// `prefix+a` referme le pane, geste symétrique de celui qui l'a ouvert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Copy,
    Clear,
    Export,
    Undo,
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
    buttons: Vec<(Command, Rect)>,
    body: Rect,
    total_rows: usize,

    dirty: bool,
    last_edit: Instant,
    last_beat: Instant,
    last_watch: Instant,
    pane_id: Option<String>,
}

impl App {
    pub fn new() -> Self {
        let mut store = Store::from_env();
        let text = store.as_mut().map(Store::load).unwrap_or_default();

        let app = Self {
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
            pane_id: std::env::var("HERDR_PANE_ID").ok().filter(|s| !s.is_empty()),
        };
        app.stamp();
        app
    }

    /// Construit une instance sans toucher au disque ni à l'environnement.
    #[cfg(test)]
    fn headless(text: &str) -> Self {
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
            pane_id: None,
        }
    }

    // -- rendu ------------------------------------------------------------

    pub fn draw(&mut self, frame: &mut Frame) {
        let status = self
            .status
            .as_ref()
            .filter(|(_, at)| at.elapsed() < STATUS_FOR)
            .map(|(text, _)| text.as_str());

        let geom = ui::draw(
            frame,
            self.buf.lines(),
            self.buf.cursor(),
            &mut self.scroll,
            status,
            self.buf.is_empty(),
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
                if let Some((command, _)) = self.buttons.iter().find(|(_, r)| r.contains(pos)) {
                    let command = *command;
                    self.run(command);
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
        self.stash = Some(text);
        self.buf = Buffer::default();
        self.scroll = 0;
        self.touch();
        // Vider est destructif : on l'écrit sur le disque tout de suite plutôt
        // que d'attendre la temporisation.
        self.flush();
        self.say("vidé · ^Z annule".into());
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
        app.buttons = vec![(Command::Clear, Rect { x: 0, y: 0, width: 8, height: 1 })];
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
        app.buttons = vec![(Command::Clear, Rect { x: 0, y: 0, width: 8, height: 1 })];
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
        app.buttons = vec![(Command::Clear, Rect { x: 0, y: 0, width: 8, height: 1 })];
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
}
