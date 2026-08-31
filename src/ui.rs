//! Rendu et géométrie des boutons.
//!
//! Règle structurante, reprise de `herdr-file-viewer` : les rectangles des
//! boutons sont calculés **une seule fois**, ici, et renvoyés à l'appelant qui
//! les rejoue au moment du clic. Le test de survol ne recalcule jamais la mise
//! en page — un clic ne peut donc pas tomber sur un bouton différent de celui
//! qui a été dessiné.

use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthChar;

use crate::agents::{self, Target};
use crate::app::{Action, Command};

/// Aide affichée quand le buffer est vide. Disparaît à la première frappe.
///
/// C'est le seul endroit qui enseigne les deux touches sans bouton, `^S` et
/// `Esc` — et le buffer vide est justement le moment où l'on referme le plus
/// volontiers. Les y faire tenir coûte le libellé long de `^E`, que le bouton
/// juste en dessous porte de toute façon en permanence : la ligne doit rester
/// sous les 80 colonnes d'un terminal étroit (§2 du DESIGN), sinon elle est
/// tronquée en son milieu.
const EMPTY_HINT: &str =
    "Paste here. ^E emit · ^C copy · ^L clear · ^S file · ^Z undo · Esc close";

/// Part de la barre réservée à la zone cible, et son plancher.
///
/// La zone porte un libellé de longueur inconnue : un agent peut s'appeler
/// autrement que `claude`. Sans budget, un libellé long ne tiendrait pas dans
/// la barre et **tout ce qui le suit** disparaîtrait — la règle de rognage
/// s'arrête au premier élément qui déborde. La zone étant maintenant la
/// dernière, il n'y a plus rien derrière elle à protéger ; le budget reste par
/// prudence, le plancher laissant passer un `→ claude·p3` entier.
const TARGET_SHARE: usize = 3;
const TARGET_MIN_COLS: usize = 13;

/// Largeur d'affichage d'une chaîne, en colonnes.
fn columns(s: &str) -> usize {
    s.chars().map(|c| UnicodeWidthChar::width(c).unwrap_or(0)).sum()
}

/// Ce que la barre sait des cibles : celle qui est retenue, et combien il y
/// en a dans la tab.
///
/// Les deux voyagent ensemble parce qu'ils se décident ensemble : c'est le
/// **nombre** qui fait apparaître `^E` puis la zone, et la cible qui remplit
/// la zone une fois qu'elle existe.
#[derive(Debug, Clone, Copy, Default)]
pub struct Targets<'a> {
    pub current: Option<&'a Target>,
    pub count: usize,
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
/// Les commandes **fixes** à gauche, les éléments **variables** à droite. La
/// liste d'agents est rafraîchie toutes les deux secondes et demie : `^E` et
/// la zone cible apparaissent et disparaissent tout seuls, au gré des agents
/// qui démarrent et s'arrêtent. À gauche, ils décaleraient `^C`, `^L` et `^Z`
/// pendant que le doigt descend vers eux ; à droite, ces trois-là ne bougent
/// **jamais** — ni selon les agents, ni selon la largeur.
///
/// `^S fichier` n'a **pas** de bouton : c'est la seule commande dont le
/// résultat n'est ni à l'écran ni dans le presse-papier mais dans un fichier
/// qu'on ira lire ailleurs — donc jamais au milieu d'un geste au doigt. La
/// touche reste, et l'aide du buffer vide l'enseigne.
///
/// Corollaire assumé : le rognage partant toujours de la droite, ce sont la
/// cible puis `^E` qui tombent d'abord sur une barre étroite. La cible est
/// maintenant locale à la tab, et `Ctrl+E` reste au clavier.
fn bar_labels(targets: Targets, bar_width: usize) -> Vec<(Action, String)> {
    let mut out = vec![
        (Action::Command(Command::Copy), "^C copy".to_owned()),
        (Action::Command(Command::Clear), "^L clear".to_owned()),
        (Action::Command(Command::Undo), "^Z undo".to_owned()),
    ];
    // Sans agent dans la tab, il n'y a rien à quoi parler : le bouton
    // n'existe pas, plutôt que d'exister pour refuser.
    if targets.count >= 1 {
        out.push((Action::Command(Command::Send), "^E emit to agent".to_owned()));
    }
    // Avec un seul agent, la zone n'apprendrait rien : elle ne dirait que ce
    // que `^E` fait déjà, et son cyclage n'aurait nulle part où aller.
    if targets.count >= 2
        && let Some(target) = targets.current
    {
        let budget = (bar_width / TARGET_SHARE).max(TARGET_MIN_COLS);
        out.push((Action::CycleTarget, agents::label(target, budget)));
    }
    out
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

/// Portion d'une ligne visuelle couverte par la sélection, en index de
/// caractères dans la ligne logique — le repère commun de `VisualRow` et des
/// bornes rendues par le buffer.
///
/// `None` quand la sélection ne touche pas ce morceau, y compris pour une
/// ligne vide au milieu d'une sélection multi-lignes : il n'y a alors aucun
/// caractère à surligner.
pub fn selection_in_row(
    visual: VisualRow,
    selection: ((usize, usize), (usize, usize)),
) -> Option<(usize, usize)> {
    let ((start_row, start_col), (end_row, end_col)) = selection;
    if visual.row < start_row || visual.row > end_row {
        return None;
    }
    let from = if visual.row == start_row { start_col } else { 0 }.max(visual.start);
    let to = if visual.row == end_row { end_col } else { usize::MAX }.min(visual.end);
    (from < to).then_some((from, to))
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

/// Position du curseur désignée par un clic dans la zone de texte.
///
/// C'est l'inverse exact du placement fait par `draw` : on repasse par `wrap`
/// avec les mêmes lignes et la même largeur, donc le clic retombe sur la
/// ligne visuelle qui a réellement été dessinée. Recalculer plutôt que
/// mémoriser reste sûr ici — contrairement aux boutons, dont les libellés
/// dépendent d'un état (cible, message) que le clic ne connaît pas.
///
/// Rend `None` quand le clic tombe hors du texte : à l'appelant de ne rien
/// faire, plutôt que de déplacer le curseur au hasard.
pub fn position_to_cursor(
    lines: &[String],
    body: Rect,
    scroll: usize,
    pos: Position,
) -> Option<(usize, usize)> {
    if !body.contains(pos) {
        return None;
    }
    let rows = wrap(lines, body.width as usize);

    // Un clic sous la dernière ligne se rabat sur elle : la zone vide du bas
    // appartient visuellement à la fin du texte.
    let index = (scroll + (pos.y - body.y) as usize).min(rows.len().saturating_sub(1));
    let visual = *rows.get(index)?;

    // La colonne visée est une colonne d'**affichage** : un idéogramme en
    // occupe deux, et compter en caractères décalerait le curseur d'autant.
    let wanted = (pos.x - body.x) as usize;
    let mut used = 0usize;
    let mut col = visual.start;
    for c in lines[visual.row]
        .chars()
        .skip(visual.start)
        .take(visual.end - visual.start)
    {
        let w = UnicodeWidthChar::width(c).unwrap_or(0);
        // Cliquer sur la seconde colonne d'un caractère large le désigne lui,
        // pas son voisin : le curseur se pose devant.
        if used + w > wanted {
            break;
        }
        used += w;
        col += 1;
    }
    // Au-delà de la fin du morceau affiché, `col == visual.end` : cliquer dans
    // le vide à droite pose le curseur en fin de ligne, comme partout.
    Some((visual.row, col))
}

/// Dispose la barre : les entrées qui tiennent, avec leur rectangle.
///
/// Une entrée qui ne tient pas entièrement n'est **pas** enregistrée, et rien
/// n'est dessiné après elle : ratatui la rognerait à l'écran, et un rectangle
/// cliquable invisible serait un piège. Comme la liste est ordonnée, ce sont
/// les entrées de droite qui tombent d'abord — la zone cible en premier, puis
/// `^E`, qui a déjà sa touche.
pub fn layout_bar(bar: Rect, targets: Targets) -> Vec<BarItem> {
    let labels = bar_labels(targets, bar.width as usize);
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

/// Ce que le rendu doit savoir du buffer : le texte, le curseur, et la
/// sélection éventuelle.
///
/// Les trois voyagent ensemble parce qu'ils décrivent le même état au même
/// instant — un curseur rendu sur d'autres lignes que les siennes serait un
/// bug de couture, pas d'affichage.
pub struct Content<'a> {
    pub lines: &'a [String],
    /// (ligne, colonne-caractère).
    pub cursor: (usize, usize),
    /// Bornes normalisées (début <= fin), même repère que le curseur.
    pub selection: Option<((usize, usize), (usize, usize))>,
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
    content: Content,
    scroll: &mut usize,
    status: Option<&str>,
    show_hint: bool,
    targets: Targets,
) -> Geometry {
    let Content { lines, cursor, selection } = content;
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
        let chars = lines[visual.row]
            .chars()
            .skip(visual.start)
            .take(visual.end - visual.start);
        // Le morceau sélectionné est rendu en vidéo inverse, découpé en trois
        // spans au plus — le patron de la barre, juste en dessous.
        let line = match selection.and_then(|s| selection_in_row(*visual, s)) {
            Some((from, to)) => {
                let (mut before, mut inside, mut after) =
                    (String::new(), String::new(), String::new());
                for (i, c) in chars.enumerate() {
                    let col = visual.start + i;
                    if col < from {
                        before.push(c);
                    } else if col < to {
                        inside.push(c);
                    } else {
                        after.push(c);
                    }
                }
                Line::from(vec![
                    Span::raw(before),
                    Span::styled(inside, Style::default().add_modifier(Modifier::REVERSED)),
                    Span::raw(after),
                ])
            }
            None => Line::raw(chars.collect::<String>()),
        };
        text.push(line);
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
        let items = layout_bar(bar, targets);
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

    /// Zone de texte de test : 10 colonnes, 3 lignes, posée en (0, 0).
    fn body() -> Rect {
        Rect { x: 0, y: 0, width: 10, height: 3 }
    }

    fn at(x: u16, y: u16) -> Position {
        Position { x, y }
    }

    /// L'aide n'est pas repliée : ce qui dépasse est coupé, et une touche
    /// coupée en deux n'enseigne rien.
    #[test]
    fn the_empty_hint_fits_a_narrow_terminal() {
        assert!(
            columns(EMPTY_HINT) <= 80,
            "l'aide déborde d'un terminal de 80 colonnes et sera tronquée : {} colonnes",
            columns(EMPTY_HINT)
        );
    }

    /// Les deux touches sans bouton n'ont que cette ligne pour se faire
    /// connaître.
    #[test]
    fn the_empty_hint_teaches_the_buttonless_keys() {
        assert!(
            EMPTY_HINT.contains("^S") && EMPTY_HINT.contains("Esc"),
            "`^S` et `Esc` n'ont pas de bouton : les retirer de l'aide les rendrait introuvables"
        );
    }

    #[test]
    fn a_click_outside_the_text_area_designates_nothing() {
        let l = lines("abc");
        assert_eq!(
            position_to_cursor(&l, body(), 0, at(0, 9)),
            None,
            "la barre du bas n'est pas du texte"
        );
    }

    #[test]
    fn a_click_designates_the_targeted_line_and_column() {
        let l = lines("abc\ndef");
        assert_eq!(position_to_cursor(&l, body(), 0, at(2, 1)), Some((1, 2)));
    }

    #[test]
    fn a_click_accounts_for_the_scroll() {
        let l = lines("a\nb\nc\nd");
        assert_eq!(
            position_to_cursor(&l, body(), 2, at(0, 0)),
            Some((2, 0)),
            "la première ligne affichée est la troisième du texte"
        );
    }

    #[test]
    fn a_click_right_of_the_text_lands_at_the_line_end() {
        let l = lines("abc");
        assert_eq!(position_to_cursor(&l, body(), 0, at(9, 0)), Some((0, 3)));
    }

    #[test]
    fn a_click_below_the_last_line_falls_back_to_it() {
        let l = lines("abc");
        assert_eq!(
            position_to_cursor(&l, body(), 0, at(1, 2)),
            Some((0, 1)),
            "le vide du bas appartient à la fin du texte"
        );
    }

    #[test]
    fn a_click_on_a_wrapped_line_targets_the_right_logical_line() {
        let l = lines("abcdefghijklm");
        let rows = wrap(&l, 10);
        assert_eq!(rows.len(), 2, "prérequis : la ligne se replie en deux");
        assert_eq!(
            position_to_cursor(&l, body(), 0, at(1, 1)),
            Some((0, 11)),
            "la deuxième ligne visuelle est la suite de la même ligne logique"
        );
    }

    #[test]
    fn a_click_counts_display_columns() {
        let l = lines("日本語");
        assert_eq!(
            position_to_cursor(&l, body(), 0, at(2, 0)),
            Some((0, 1)),
            "un idéogramme occupe deux colonnes, pas une"
        );
    }

    #[test]
    fn a_click_on_the_second_column_of_a_wide_char_designates_that_char() {
        let l = lines("日本語");
        assert_eq!(position_to_cursor(&l, body(), 0, at(1, 0)), Some((0, 0)));
    }

    #[test]
    fn a_click_on_an_empty_line_stays_at_column_zero() {
        let l = lines("\nb");
        assert_eq!(position_to_cursor(&l, body(), 0, at(7, 0)), Some((0, 0)));
    }

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

    // -- sélection ---------------------------------------------------------

    #[test]
    fn selection_inside_a_single_row_keeps_its_bounds() {
        let visual = VisualRow { row: 0, start: 0, end: 5 };
        assert_eq!(selection_in_row(visual, ((0, 1), (0, 3))), Some((1, 3)));
    }

    #[test]
    fn selection_straddling_the_wrap_covers_both_chunks() {
        let sel = ((0, 1), (0, 3));
        assert_eq!(
            selection_in_row(VisualRow { row: 0, start: 0, end: 2 }, sel),
            Some((1, 2)),
            "le premier morceau porte le début de la sélection"
        );
        assert_eq!(
            selection_in_row(VisualRow { row: 0, start: 2, end: 4 }, sel),
            Some((2, 3)),
            "le second morceau porte la fin"
        );
    }

    #[test]
    fn a_middle_line_is_fully_covered() {
        let visual = VisualRow { row: 1, start: 0, end: 4 };
        assert_eq!(
            selection_in_row(visual, ((0, 2), (2, 1))),
            Some((0, 4)),
            "une ligne traversée de part en part est surlignée entière"
        );
    }

    #[test]
    fn an_empty_line_inside_the_selection_has_nothing_to_highlight() {
        let visual = VisualRow { row: 1, start: 0, end: 0 };
        assert_eq!(selection_in_row(visual, ((0, 0), (2, 1))), None);
    }

    #[test]
    fn a_row_outside_the_selection_is_untouched() {
        let sel = ((1, 0), (1, 2));
        assert_eq!(selection_in_row(VisualRow { row: 0, start: 0, end: 4 }, sel), None);
        assert_eq!(selection_in_row(VisualRow { row: 2, start: 0, end: 4 }, sel), None);
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

    fn target(agent: &str, pane_id: &str) -> Target {
        Target { pane_id: pane_id.into(), agent: agent.into() }
    }

    /// Vue d'une tab à `count` agents, dont `claude` est la cible retenue.
    fn view(current: &Target, count: usize) -> Targets<'_> {
        Targets { current: Some(current), count }
    }

    /// La barre d'une tab à deux agents : tout est là.
    fn bar(width: u16) -> Vec<BarItem> {
        let t = target("claude", "w2:p1");
        layout_bar(Rect { x: 0, y: 0, width, height: 1 }, view(&t, 2))
    }

    /// Les actions d'une barre large, pour un nombre d'agents donné.
    fn actions(target_count: usize) -> Vec<Action> {
        let t = target("claude", "w2:p1");
        layout_bar(Rect { x: 0, y: 0, width: 120, height: 1 }, view(&t, target_count))
            .iter()
            .map(|i| i.action)
            .collect()
    }

    /// Les rectangles des trois commandes fixes, pour un nombre d'agents donné.
    fn fixed_rects(target_count: usize) -> Vec<Rect> {
        let t = target("claude", "w2:p1");
        layout_bar(Rect { x: 0, y: 0, width: 120, height: 1 }, view(&t, target_count))
            .iter()
            .filter(|i| {
                matches!(
                    i.action,
                    Action::Command(Command::Copy)
                        | Action::Command(Command::Clear)
                        | Action::Command(Command::Undo)
                )
            })
            .map(|i| i.rect)
            .collect()
    }

    #[test]
    fn buttons_are_laid_out_left_to_right_without_overlap() {
        let t = target("claude", "w2:p1");
        let items = layout_bar(Rect { x: 0, y: 9, width: 120, height: 1 }, view(&t, 2));
        assert_eq!(items.len(), 5);
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
        assert_eq!(items.len(), 1, "seul « ^C copy » (7) tient dans 12");

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
        let t = target("claude", "w2:p1");
        let items = layout_bar(Rect { x: 5, y: 0, width: 120, height: 1 }, view(&t, 2));
        assert_eq!(items[0].rect.x, 5);
    }

    #[test]
    fn button_order_matches_the_declared_one() {
        assert_eq!(
            actions(2),
            vec![
                Action::Command(Command::Copy),
                Action::Command(Command::Clear),
                Action::Command(Command::Undo),
                Action::Command(Command::Send),
                Action::CycleTarget,
            ]
        );
    }

    /// Sans agent dans la tab, il n'y a rien à quoi parler : ni le bouton, ni
    /// la zone.
    #[test]
    fn without_an_agent_neither_send_nor_target_area_are_in_the_bar() {
        assert_eq!(
            actions(0),
            vec![
                Action::Command(Command::Copy),
                Action::Command(Command::Clear),
                Action::Command(Command::Undo),
            ]
        );
    }

    /// Avec un seul agent, la zone n'apprendrait rien — `^E` dit déjà tout.
    #[test]
    fn with_one_agent_send_is_there_but_not_the_target_area() {
        assert_eq!(
            actions(1),
            vec![
                Action::Command(Command::Copy),
                Action::Command(Command::Clear),
                Action::Command(Command::Undo),
                Action::Command(Command::Send),
            ]
        );
    }

    #[test]
    fn with_two_agents_send_and_the_target_area_are_both_there() {
        assert!(actions(2).contains(&Action::Command(Command::Send)));
        assert!(actions(2).contains(&Action::CycleTarget));
    }

    /// La raison d'être de l'inversion : un agent qui démarre ou s'arrête ne
    /// doit pas déplacer les trois commandes sous le doigt qui descend.
    #[test]
    fn the_fixed_commands_do_not_move_with_the_agent_count() {
        assert_eq!(fixed_rects(0), fixed_rects(1), "un agent qui démarre ne décale rien");
        assert_eq!(fixed_rects(1), fixed_rects(2), "un second non plus");
    }

    /// Corollaire assumé de l'inversion : sur une barre étroite, ce sont la
    /// cible puis `^E` qui tombent — jamais les trois commandes fixes.
    #[test]
    fn the_fixed_commands_survive_a_narrow_bar() {
        let items = bar(30);
        let actions: Vec<_> = items.iter().map(|i| i.action).collect();
        assert!(actions.starts_with(&[
            Action::Command(Command::Copy),
            Action::Command(Command::Clear),
            Action::Command(Command::Undo),
        ]));
        assert!(!actions.contains(&Action::CycleTarget), "la zone tombe la première");
    }

    /// `Ctrl+S` garde sa touche et sa fonction, mais pas de bouton : son
    /// résultat est un fichier qu'on ira lire ailleurs, jamais au milieu d'un
    /// geste au doigt.
    #[test]
    fn export_has_no_button_in_any_of_the_three_shapes() {
        for count in 0..=2 {
            assert!(
                !actions(count).contains(&Action::Command(Command::Export)),
                "{count} agent(s)"
            );
        }
    }

    /// Un nom d'agent interminable ne doit pas emporter le reste de la barre
    /// avec lui : la zone cible a un budget.
    #[test]
    fn an_endless_agent_name_does_not_eat_the_bar() {
        let long = target(&"a".repeat(200), "w2:p1");
        let items = layout_bar(Rect { x: 0, y: 0, width: 120, height: 1 }, view(&long, 2));
        assert_eq!(items.len(), 5);
    }

    /// Deux agents annoncés mais aucune cible retenue : la barre ne doit pas
    /// s'effondrer, elle affiche juste ce qu'elle sait.
    #[test]
    fn the_bar_still_lays_out_without_a_selected_target() {
        let items = layout_bar(
            Rect { x: 0, y: 0, width: 120, height: 1 },
            Targets { current: None, count: 2 },
        );
        assert_eq!(items.len(), 4);
        assert!(!items.iter().any(|i| i.action == Action::CycleTarget));
    }
}
