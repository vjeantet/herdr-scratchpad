//! Rendu et géométrie des boutons.
//!
//! Règle structurante, reprise de `herdr-file-viewer` : les rectangles des
//! boutons sont calculés **une seule fois**, ici, et renvoyés à l'appelant qui
//! les rejoue au moment du clic. Le test de survol ne recalcule jamais la mise
//! en page — un clic ne peut donc pas tomber sur un bouton différent de celui
//! qui a été dessiné.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthChar;

use crate::agents::{self, Target};
use crate::app::{Action, Command};

/// Aide affichée quand le buffer est vide. Disparaît à la première frappe.
const EMPTY_HINT: &str =
    "Colle ici. ^E envoyer · ^C copier · ^L vider · ^S fichier · ^Z annuler";

/// Part de la barre réservée à la zone cible, et son plancher.
///
/// La zone porte un libellé de longueur inconnue (un workspace peut s'appeler
/// `herdr-scratchpad`). Sans budget, un libellé long ne tiendrait pas dans la
/// barre et **tout ce qui le suit** disparaîtrait — la règle de rognage
/// s'arrête au premier élément qui déborde. Le plancher laisse toujours passer
/// `→ aucun agent`.
const TARGET_SHARE: usize = 3;
const TARGET_MIN_COLS: usize = 13;

/// Largeur d'affichage d'une chaîne, en colonnes.
fn columns(s: &str) -> usize {
    s.chars().map(|c| UnicodeWidthChar::width(c).unwrap_or(0)).sum()
}

/// Une entrée de la barre du bas : ce qu'elle fait, ce qu'elle affiche, où.
///
/// Les trois voyagent **ensemble**. Le rendu et le test de survol lisent la
/// même liste : un libellé ne peut donc pas se retrouver dessiné sur le
/// rectangle d'une autre action, ce que deux tableaux parallèles finissaient
/// toujours par produire.
pub struct BarItem {
    pub action: Action,
    pub label: String,
    pub rect: Rect,
}

/// Les entrées de la barre, dans l'ordre d'affichage.
///
/// `envoyer` et sa cible passent en tête : c'est la seule commande sortante du
/// plugin, et sa destination doit être lisible avant qu'on appuie. Le
/// destructif n'est toujours pas au bord — les extrémités sont là où le pouce
/// dérape sur un petit écran.
fn bar_labels(target: Option<&Target>, bar_width: usize) -> Vec<(Action, String)> {
    let budget = (bar_width / TARGET_SHARE).max(TARGET_MIN_COLS);
    vec![
        (Action::Command(Command::Send), "^E envoyer".to_owned()),
        (Action::CycleTarget, agents::label(target, budget)),
        (Action::Command(Command::Copy), "^C copier".to_owned()),
        (Action::Command(Command::Clear), "^L vider".to_owned()),
        (Action::Command(Command::Export), "^S fichier".to_owned()),
        (Action::Command(Command::Undo), "^Z annuler".to_owned()),
    ]
}

/// Une ligne visuelle : un morceau d'une ligne logique, tel qu'il est affiché.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisualRow {
    /// Ligne logique d'origine.
    pub row: usize,
    /// Premier caractère affiché, en index de caractère dans la ligne logique.
    pub start: usize,
    /// Premier caractère **non** affiché.
    pub end: usize,
}

/// Découpe les lignes logiques en lignes visuelles pour une largeur donnée.
///
/// Le repliage se fait en **colonnes d'affichage** et non en caractères : un
/// emoji ou un idéogramme occupe deux colonnes, et compter en caractères
/// laisserait déborder la dernière colonne du pane.
///
/// Une ligne vide produit une ligne visuelle vide — sinon elle disparaîtrait
/// de l'affichage.
pub fn wrap(lines: &[String], width: usize) -> Vec<VisualRow> {
    let width = width.max(1);
    let mut out = Vec::new();

    for (row, line) in lines.iter().enumerate() {
        let mut start = 0usize;
        let mut used = 0usize;
        let mut count = 0usize;

        for c in line.chars() {
            // Un caractère plus large que le pane ne peut jamais tenir : on le
            // laisse déborder seul sur sa ligne plutôt que de boucler.
            let w = UnicodeWidthChar::width(c).unwrap_or(0);
            if used + w > width && count > start {
                out.push(VisualRow { row, start, end: count });
                start = count;
                used = 0;
            }
            used += w;
            count += 1;
        }
        out.push(VisualRow { row, start, end: count });
    }
    out
}

/// Index de la ligne visuelle qui porte le curseur.
pub fn cursor_visual_row(rows: &[VisualRow], cursor: (usize, usize)) -> usize {
    let (row, col) = cursor;
    rows.iter()
        .position(|v| v.row == row && col >= v.start && col < v.end)
        // En bout de ligne, `col == end` : on retombe sur le dernier morceau
        // de cette ligne logique.
        .or_else(|| rows.iter().rposition(|v| v.row == row))
        .unwrap_or(0)
}

/// Dispose la barre : les entrées qui tiennent, avec leur rectangle.
///
/// Une entrée qui ne tient pas entièrement n'est **pas** enregistrée, et rien
/// n'est dessiné après elle : ratatui la rognerait à l'écran, et un rectangle
/// cliquable invisible serait un piège. Comme la liste est ordonnée, ce sont
/// les entrées de droite qui tombent d'abord — `^Z annuler` en premier, il a
/// déjà sa touche.
pub fn layout_bar(bar: Rect, target: Option<&Target>) -> Vec<BarItem> {
    let labels = bar_labels(target, bar.width as usize);
    let mut out = Vec::with_capacity(labels.len());
    let mut x = bar.x;
    let right = bar.x.saturating_add(bar.width);

    for (action, label) in labels {
        let w = columns(&label) as u16;
        if x.saturating_add(w) > right {
            break;
        }
        out.push(BarItem {
            action,
            label,
            rect: Rect { x, y: bar.y, width: w, height: 1 },
        });
        // Une colonne de séparation, non cliquable.
        x = x.saturating_add(w).saturating_add(1);
    }
    out
}

/// Ce que le rendu renvoie à l'application pour le prochain événement souris.
pub struct Geometry {
    /// Zone de texte.
    pub body: Rect,
    /// Entrées de barre effectivement dessinées.
    pub buttons: Vec<(Action, Rect)>,
    /// Nombre total de lignes visuelles, pour borner le défilement.
    pub total_rows: usize,
}

/// Dessine le pane et rend sa géométrie.
///
/// `scroll` est en lignes **visuelles**. Il est écrêté ici, contre la hauteur
/// réellement disponible, et l'appelant récupère la valeur corrigée.
pub fn draw(
    frame: &mut Frame,
    lines: &[String],
    cursor: (usize, usize),
    scroll: &mut usize,
    status: Option<&str>,
    show_hint: bool,
    target: Option<&Target>,
) -> Geometry {
    let area = frame.area();

    // La barre occupe exactement une ligne — le coût fixe assumé du design.
    // Sur un pane d'une seule ligne, le texte passe avant la barre.
    let (body, bar) = if area.height >= 2 {
        (
            Rect { height: area.height - 1, ..area },
            Rect { y: area.y + area.height - 1, height: 1, ..area },
        )
    } else {
        (area, Rect { height: 0, ..area })
    };

    let rows = wrap(lines, body.width as usize);
    let height = body.height as usize;

    // Garder le curseur visible prime sur la position de défilement demandée.
    let cursor_row = cursor_visual_row(&rows, cursor);
    let max_scroll = rows.len().saturating_sub(height);
    *scroll = (*scroll).min(max_scroll);
    if cursor_row < *scroll {
        *scroll = cursor_row;
    } else if height > 0 && cursor_row >= *scroll + height {
        *scroll = cursor_row + 1 - height;
    }

    let mut text: Vec<Line> = Vec::with_capacity(height);
    for visual in rows.iter().skip(*scroll).take(height) {
        let slice: String = lines[visual.row]
            .chars()
            .skip(visual.start)
            .take(visual.end - visual.start)
            .collect();
        text.push(Line::raw(slice));
    }

    if show_hint && text.first().is_some_and(|l| l.width() == 0) {
        text[0] = Line::styled(EMPTY_HINT, Style::default().fg(Color::DarkGray));
    }

    frame.render_widget(Paragraph::new(text), body);

    let buttons = if bar.height == 0 {
        Vec::new()
    } else if let Some(message) = status {
        // Le retour prend la place des boutons pendant quelques secondes : on
        // ne reclique pas dans la seconde, et ça économise une ligne à demeure.
        frame.render_widget(
            Paragraph::new(Line::styled(
                message,
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            )),
            bar,
        );
        Vec::new()
    } else {
        let items = layout_bar(bar, target);
        let mut spans = Vec::with_capacity(items.len() * 2);
        for (i, item) in items.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw(" "));
            }
            // La zone cible se distingue du bouton d'envoi : l'une agit,
            // l'autre informe — et c'est cet affichage qui tient lieu de
            // garde-fou, faute de confirmation.
            let style = match item.action {
                Action::CycleTarget => Style::default().fg(Color::Black).bg(Color::Cyan),
                Action::Command(_) => Style::default().fg(Color::Black).bg(Color::Gray),
            };
            spans.push(Span::styled(item.label.clone(), style));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), bar);
        items.into_iter().map(|i| (i.action, i.rect)).collect()
    };

    // Le curseur est posé à sa colonne d'affichage, pas à son index de
    // caractère : accents, emoji et idéogrammes le décaleraient sinon.
    if height > 0 && cursor_row >= *scroll && cursor_row < *scroll + height {
        let visual = rows[cursor_row];
        let width: usize = lines[visual.row]
            .chars()
            .skip(visual.start)
            .take(cursor.1.saturating_sub(visual.start))
            .map(|c| UnicodeWidthChar::width(c).unwrap_or(0))
            .sum();
        frame.set_cursor_position((
            body.x + (width as u16).min(body.width.saturating_sub(1)),
            body.y + (cursor_row - *scroll) as u16,
        ));
    }

    Geometry {
        body,
        buttons,
        total_rows: rows.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.split('\n').map(str::to_owned).collect()
    }

    #[test]
    fn short_lines_are_not_wrapped() {
        let rows = wrap(&lines("abc\nde"), 10);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], VisualRow { row: 0, start: 0, end: 3 });
        assert_eq!(rows[1], VisualRow { row: 1, start: 0, end: 2 });
    }

    #[test]
    fn long_line_wraps_at_the_width() {
        let rows = wrap(&lines("abcdef"), 2);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], VisualRow { row: 0, start: 0, end: 2 });
        assert_eq!(rows[2], VisualRow { row: 0, start: 4, end: 6 });
    }

    #[test]
    fn empty_line_still_produces_a_visual_row() {
        let rows = wrap(&lines("a\n\nb"), 10);
        assert_eq!(rows.len(), 3, "la ligne vide doit rester visible");
        assert_eq!(rows[1], VisualRow { row: 1, start: 0, end: 0 });
    }

    /// Un idéogramme fait deux colonnes : trois d'entre eux ne tiennent pas
    /// dans une largeur de 4, même s'ils ne font que trois caractères.
    #[test]
    fn wrapping_counts_display_columns_not_chars() {
        let rows = wrap(&lines("日本語"), 4);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].end, 2, "deux idéogrammes = 4 colonnes");
    }

    #[test]
    fn a_char_wider_than_the_pane_does_not_loop() {
        let rows = wrap(&lines("日本"), 1);
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn zero_width_is_treated_as_one() {
        let rows = wrap(&lines("ab"), 0);
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn cursor_at_line_end_lands_on_the_last_chunk() {
        let rows = wrap(&lines("abcdef"), 2);
        assert_eq!(cursor_visual_row(&rows, (0, 6)), 2);
    }

    #[test]
    fn cursor_inside_a_chunk_lands_on_that_chunk() {
        let rows = wrap(&lines("abcdef"), 2);
        assert_eq!(cursor_visual_row(&rows, (0, 3)), 1);
    }

    #[test]
    fn cursor_on_an_empty_line_is_found() {
        let rows = wrap(&lines("a\n\nb"), 10);
        assert_eq!(cursor_visual_row(&rows, (1, 0)), 1);
    }

    fn target() -> Target {
        Target {
            pane_id: "w2:p1".into(),
            agent: "claude".into(),
            tab_id: "w2:t1".into(),
            workspace_id: "w2".into(),
            workspace_label: "wdv".into(),
        }
    }

    fn bar(width: u16) -> Vec<BarItem> {
        layout_bar(Rect { x: 0, y: 0, width, height: 1 }, Some(&target()))
    }

    #[test]
    fn buttons_are_laid_out_left_to_right_without_overlap() {
        let items = layout_bar(Rect { x: 0, y: 9, width: 120, height: 1 }, Some(&target()));
        assert_eq!(items.len(), 6);
        for pair in items.windows(2) {
            assert!(
                pair[0].rect.x + pair[0].rect.width < pair[1].rect.x,
                "les boutons doivent être disjoints"
            );
        }
        assert!(items.iter().all(|i| i.rect.y == 9 && i.rect.height == 1));
    }

    /// Un bouton rogné par ratatui ne doit pas rester cliquable : on
    /// enregistrerait une cible invisible.
    #[test]
    fn buttons_that_do_not_fit_are_dropped() {
        let items = bar(12);
        assert_eq!(items.len(), 1, "seul « ^E envoyer » (10) tient dans 12");

        assert!(bar(3).is_empty());
    }

    /// Le rectangle enregistré doit faire exactement la largeur du libellé
    /// dessiné, sinon un clic tomberait à côté.
    #[test]
    fn each_rect_matches_the_width_of_its_own_label() {
        for item in bar(120) {
            assert_eq!(item.rect.width as usize, columns(&item.label), "{}", item.label);
        }
    }

    #[test]
    fn buttons_respect_a_non_zero_origin() {
        let items = layout_bar(Rect { x: 5, y: 0, width: 120, height: 1 }, Some(&target()));
        assert_eq!(items[0].rect.x, 5);
    }

    #[test]
    fn button_order_matches_the_declared_one() {
        let actions: Vec<_> = bar(120).iter().map(|i| i.action).collect();
        assert_eq!(
            actions,
            vec![
                Action::Command(Command::Send),
                Action::CycleTarget,
                Action::Command(Command::Copy),
                Action::Command(Command::Clear),
                Action::Command(Command::Export),
                Action::Command(Command::Undo),
            ]
        );
    }

    /// Le rognage part de la droite : sur une barre serrée, `envoyer` et la
    /// cible survivent — on ne doit jamais pouvoir envoyer sans lire où.
    #[test]
    fn send_and_its_target_survive_a_narrow_bar() {
        let items = bar(24);
        assert!(items.len() >= 2, "il reste {} entrée(s)", items.len());
        assert_eq!(items[0].action, Action::Command(Command::Send));
        assert_eq!(items[1].action, Action::CycleTarget);
        assert!(items[1].label.contains("claude"));
    }

    /// Un libellé de workspace interminable ne doit pas emporter le reste de
    /// la barre avec lui : la zone cible a un budget.
    #[test]
    fn a_very_long_workspace_label_does_not_eat_the_bar() {
        let mut long = target();
        long.workspace_label = "w".repeat(200);
        let items = layout_bar(Rect { x: 0, y: 0, width: 120, height: 1 }, Some(&long));
        assert_eq!(items.len(), 6);
    }

    #[test]
    fn the_bar_still_lays_out_without_a_target() {
        let items = layout_bar(Rect { x: 0, y: 0, width: 120, height: 1 }, None);
        assert_eq!(items.len(), 6);
        assert_eq!(items[1].label, agents::NO_TARGET);
    }
}
