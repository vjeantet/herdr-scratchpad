//! herdr-scratchpad — un presse-papier éditable persistant dans un pane herdr.
//!
//! Le binaire a deux vies. Sans argument, il est la TUI du pane. Avec un
//! drapeau, il répond à une question du script lanceur et sort — ce qui garde
//! toute la logique de décision en Rust testable plutôt qu'en shell.

mod agents;
mod app;
mod buffer;
mod clipboard;
mod ipc;
mod launch;
mod state;
mod ui;

use std::io::{self, Read};
use std::time::Duration;

use crossterm::event::{
    self, DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture, Event,
};
use crossterm::execute;

/// Période d'attente d'un événement.
///
/// Elle borne la réactivité des horloges (sauvegarde, estampille,
/// surveillance du fichier), pas seulement celle du clavier.
const POLL: Duration = Duration::from_millis(250);

fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match args.first().map(String::as_str) {
        Some("--launch-decision") => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            println!("{}", launch::launch_decision(&read_stdin(), now));
            Ok(())
        }
        Some("--focused-pane") => {
            println!("{}", launch::focused_pane(&read_stdin()));
            Ok(())
        }
        Some("--open-plan") => {
            println!("{}", launch::open_plan(&read_stdin()));
            Ok(())
        }
        Some("--stamp") => {
            if let Some(pane_id) = args.get(1) {
                ipc::stamp(pane_id);
            }
            Ok(())
        }
        _ => tui(),
    }
}

fn read_stdin() -> String {
    let mut input = String::new();
    let _ = io::stdin().read_to_string(&mut input);
    input
}

fn tui() -> io::Result<()> {
    let mut terminal = ratatui::try_init()?;

    // `ratatui::init` ne met en place que le mode brut et l'écran alterné.
    // Trois modes de plus sont nécessaires ici :
    //
    // - la **capture souris**, pour les boutons cliquables ;
    // - le **bracketed paste**, sans lequel un collage de 50 Ko arriverait
    //   touche par touche — or coller est l'entrée principale de cet outil ;
    // - le **suivi de focus**, pour sauvegarder quand le pane perd la main
    //   (`herdr pane close` tue le processus sans signal).
    //
    // Le tout en « au mieux » : un terminal qui n'en supporte pas un doit
    // continuer de fonctionner sans lui.
    let _ = execute!(
        io::stdout(),
        EnableMouseCapture,
        EnableBracketedPaste,
        EnableFocusChange
    );

    // Le hook de ratatui restaure le terminal mais ignore les modes qu'on
    // vient d'activer : sans ce maillon devant lui, une panique laisserait le
    // terminal de l'utilisateur bloqué en mode rapport-souris.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(
            io::stdout(),
            DisableMouseCapture,
            DisableBracketedPaste,
            DisableFocusChange
        );
        previous(info);
    }));

    let mut app = app::App::new();
    let result = run(&mut terminal, &mut app);

    // Sauvegarder avant de rendre le terminal : l'ordre inverse perdrait le
    // texte si la restauration échouait.
    app.finalize();
    let _ = execute!(
        io::stdout(),
        DisableMouseCapture,
        DisableBracketedPaste,
        DisableFocusChange
    );
    ratatui::try_restore()?;
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut app::App) -> io::Result<()> {
    loop {
        terminal.draw(|frame| app.draw(frame))?;

        if event::poll(POLL)? {
            match event::read()? {
                Event::Key(key) => app.on_key(key),
                Event::Paste(text) => app.on_paste(&text),
                Event::Mouse(mouse) => app.on_mouse(mouse),
                Event::FocusLost => app.on_focus_lost(),
                // Redimensionnement et reprise de focus : le redessin de
                // début de boucle suffit.
                _ => {}
            }
        }

        // Pompées à **chaque** tour, pas seulement quand l'attente expire :
        // une saisie soutenue (répétition de touche, long collage) affamerait
        // sinon l'estampille jusqu'à ce que le lanceur déclare le pane mort et
        // le remplace en pleine frappe.
        app.maybe_flush();
        app.maybe_reload();
        app.maybe_refresh_targets();
        app.heartbeat();
    }
}
