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

use crate::app::Command;

/// Les boutons de la barre du bas, dans l'ordre d'affichage.
///
/// Le destructif n'est pas au bord : les extrémités sont là où le pouce
/// dérape sur un petit écran.
pub const BUTTONS: [(Command, &str); 4] = [
    (Command::Copy, "^C copier"),
    (Command::Clear, "^L vider"),
    (Command::Export, "^S fichier"),
    (Command::Undo, "^Z annuler"),
];

/// Aide affichée quand le buffer est vide. Disparaît à la première frappe.
const EMPTY_HINT: &str = "Colle ici. ^C copier · ^L vider · ^S fichier · ^Z annuler";

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

/// Rectangles des boutons de la barre, dans l'ordre.
///
/// Un bouton qui ne tient pas entièrement n'est **pas** enregistré : ratatui
/// le rogne à l'écran, et un rectangle cliquable invisible serait un piège.
pub fn button_rects(bar: Rect) -> Vec<(Command, Rect)> {
    let mut out = Vec::with_capacity(BUTTONS.len());
    let mut x = bar.x;
    let right = bar.x.saturating_add(bar.width);

    for (command, label) in BUTTONS {
        let w = label.chars().count() as u16;
        if x.saturating_add(w) > right {
            break;
        }
        out.push((
            command,
            Rect {
                x,
                y: bar.y,
                width: w,
                height: 1,
            },
        ));
        // Une colonne de séparation, non cliquable.
        x = x.saturating_add(w).saturating_add(1);
    }
    out
}

/// Ce que le rendu renvoie à l'application pour le prochain événement souris.
pub struct Geometry {
    /// Zone de texte.
    pub body: Rect,
    /// Boutons effectivement dessinés.
    pub buttons: Vec<(Command, Rect)>,
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
        let rects = button_rects(bar);
        let mut spans = Vec::with_capacity(rects.len() * 2);
        for (i, (_, rect)) in rects.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw(" "));
            }
            spans.push(Span::styled(
                BUTTONS[i].1,
                Style::default().fg(Color::Black).bg(Color::Gray),
            ));
            let _ = rect;
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), bar);
        rects
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

    #[test]
    fn buttons_are_laid_out_left_to_right_without_overlap() {
        let rects = button_rects(Rect { x: 0, y: 9, width: 80, height: 1 });
        assert_eq!(rects.len(), 4);
        for pair in rects.windows(2) {
            let (_, a) = pair[0];
            let (_, b) = pair[1];
            assert!(a.x + a.width < b.x, "les boutons doivent être disjoints");
        }
        assert!(rects.iter().all(|(_, r)| r.y == 9 && r.height == 1));
    }

    /// Un bouton rogné par ratatui ne doit pas rester cliquable : on
    /// enregistrerait une cible invisible.
    #[test]
    fn buttons_that_do_not_fit_are_dropped() {
        let rects = button_rects(Rect { x: 0, y: 0, width: 12, height: 1 });
        assert_eq!(rects.len(), 1, "seul « ^C copier » (9) tient dans 12");

        let rects = button_rects(Rect { x: 0, y: 0, width: 3, height: 1 });
        assert!(rects.is_empty());
    }

    #[test]
    fn buttons_respect_a_non_zero_origin() {
        let rects = button_rects(Rect { x: 5, y: 0, width: 80, height: 1 });
        assert_eq!(rects[0].1.x, 5);
    }

    #[test]
    fn button_order_matches_the_declared_one() {
        let rects = button_rects(Rect { x: 0, y: 0, width: 80, height: 1 });
        let commands: Vec<_> = rects.iter().map(|(c, _)| *c).collect();
        assert_eq!(
            commands,
            vec![Command::Copy, Command::Clear, Command::Export, Command::Undo]
        );
    }
}
