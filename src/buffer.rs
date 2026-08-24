//! Le texte et le curseur.
//!
//! Édition **minimale** et assumée comme telle : flèches, `Backspace`,
//! `Suppr`, `Home`/`End`, `PgUp`/`PgDn`, `Entrée`. Pas de readline
//! (`Ctrl+A/E/K/W/U`) — ces combinaisons servent aux commandes, qui sont
//! utilisées tous les jours là où l'édition fine l'est une fois par mois.
//!
//! Corollaire : pas d'undo de frappe. `Ctrl+Z` appartient entièrement au
//! rattrapage du vidage, géré plus haut.

/// Nombre de lignes parcourues par `PgUp` / `PgDn` à défaut de connaître la
/// hauteur du pane (le rendu la fournit quand il l'a).
pub const PAGE_FALLBACK: usize = 10;

/// Texte multiligne avec un curseur.
///
/// Le texte est conservé en lignes logiques ; le retour à la ligne visuel est
/// une affaire de rendu. Les déplacements verticaux suivent donc la ligne
/// **logique**, pas la ligne affichée : sur du texte collé — qui porte ses
/// propres retours à la ligne — les deux coïncident presque toujours.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Buffer {
    lines: Vec<String>,
    /// Ligne du curseur, toujours < `lines.len()`.
    row: usize,
    /// Colonne du curseur, en **caractères** (pas en octets), toujours <= la
    /// longueur de la ligne courante.
    col: usize,
}

impl Default for Buffer {
    fn default() -> Self {
        Self {
            lines: vec![String::new()],
            row: 0,
            col: 0,
        }
    }
}

impl Buffer {
    /// Construit un buffer à partir d'un texte, curseur à la fin.
    ///
    /// La fin est le bon endroit : on ouvre le scratchpad pour ajouter, et ça
    /// rend le collage « à la suite » naturel sans commande dédiée.
    pub fn from_text(text: &str) -> Self {
        let mut buf = Self {
            lines: split_lines(text),
            row: 0,
            col: 0,
        };
        buf.cursor_to_end();
        buf
    }

    /// Remplace le contenu en préservant la position du curseur autant que
    /// possible — utilisé au rechargement quand un agent a écrit le fichier
    /// sous nos pieds.
    pub fn replace_preserving_cursor(&mut self, text: &str) {
        self.lines = split_lines(text);
        self.clamp_cursor();
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn is_empty(&self) -> bool {
        self.lines.len() == 1 && self.lines[0].is_empty()
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// Position du curseur, en (ligne, colonne-caractère).
    pub fn cursor(&self) -> (usize, usize) {
        (self.row, self.col)
    }

    // -- édition ----------------------------------------------------------

    pub fn insert_char(&mut self, c: char) {
        if c == '\n' {
            self.insert_newline();
            return;
        }
        let byte = self.byte_offset(self.row, self.col);
        self.lines[self.row].insert(byte, c);
        self.col += 1;
    }

    pub fn insert_newline(&mut self) {
        let byte = self.byte_offset(self.row, self.col);
        let tail = self.lines[self.row].split_off(byte);
        self.lines.insert(self.row + 1, tail);
        self.row += 1;
        self.col = 0;
    }

    /// Insère un texte collé d'un bloc.
    ///
    /// Le *bracketed paste* de herdr livre le collage en un seul événement
    /// (`src/pane.rs:2858`), donc on l'insère en une fois : coller 50 Ko ne
    /// coûte pas 50 000 insertions.
    ///
    /// Les `\r` sont normalisés — un collage venu de Windows ou d'un terminal
    /// en mode raw en charrie, et ils s'afficheraient comme des parasites.
    pub fn insert_str(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let incoming = split_lines(&text.replace("\r\n", "\n").replace('\r', "\n"));

        let byte = self.byte_offset(self.row, self.col);
        let tail = self.lines[self.row].split_off(byte);

        let (first, rest) = incoming.split_first().expect("split_lines never empties");
        self.lines[self.row].push_str(first);

        if rest.is_empty() {
            self.col += first.chars().count();
            self.lines[self.row].push_str(&tail);
        } else {
            let last_len = rest[rest.len() - 1].chars().count();
            for (i, line) in rest.iter().enumerate() {
                self.lines.insert(self.row + 1 + i, line.clone());
            }
            self.row += rest.len();
            self.col = last_len;
            self.lines[self.row].push_str(&tail);
        }
    }

    pub fn backspace(&mut self) {
        if self.col > 0 {
            let byte = self.byte_offset(self.row, self.col - 1);
            self.lines[self.row].remove(byte);
            self.col -= 1;
        } else if self.row > 0 {
            let current = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.lines[self.row].chars().count();
            self.lines[self.row].push_str(&current);
        }
    }

    pub fn delete(&mut self) {
        let len = self.lines[self.row].chars().count();
        if self.col < len {
            let byte = self.byte_offset(self.row, self.col);
            self.lines[self.row].remove(byte);
        } else if self.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.row + 1);
            self.lines[self.row].push_str(&next);
        }
    }

    // -- déplacements -----------------------------------------------------

    pub fn left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.lines[self.row].chars().count();
        }
    }

    pub fn right(&mut self) {
        if self.col < self.lines[self.row].chars().count() {
            self.col += 1;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    pub fn up(&mut self, n: usize) {
        self.row = self.row.saturating_sub(n);
        self.clamp_col();
    }

    pub fn down(&mut self, n: usize) {
        self.row = (self.row + n).min(self.lines.len().saturating_sub(1));
        self.clamp_col();
    }

    pub fn home(&mut self) {
        self.col = 0;
    }

    pub fn end(&mut self) {
        self.col = self.lines[self.row].chars().count();
    }

    pub fn cursor_to_end(&mut self) {
        self.row = self.lines.len().saturating_sub(1);
        self.end();
    }

    // -- interne ----------------------------------------------------------

    fn clamp_col(&mut self) {
        self.col = self.col.min(self.lines[self.row].chars().count());
    }

    fn clamp_cursor(&mut self) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.row = self.row.min(self.lines.len() - 1);
        self.clamp_col();
    }

    /// Convertit une colonne-caractère en décalage d'octets.
    ///
    /// Indispensable : `String::insert`/`remove` travaillent en octets, et un
    /// scratchpad reçoit des accents, des emoji et des chemins UTF-8.
    fn byte_offset(&self, row: usize, col: usize) -> usize {
        self.lines[row]
            .char_indices()
            .nth(col)
            .map(|(i, _)| i)
            .unwrap_or(self.lines[row].len())
    }
}

/// Découpe en lignes, en garantissant au moins une ligne.
///
/// `"".split('\n')` rend `[""]`, ce qui est exactement voulu : un buffer vide
/// a une ligne vide, jamais zéro ligne — tout le reste du module suppose
/// `lines[row]` valide.
fn split_lines(text: &str) -> Vec<String> {
    text.split('\n').map(str::to_owned).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_buffer_has_one_empty_line() {
        let buf = Buffer::default();
        assert!(buf.is_empty());
        assert_eq!(buf.lines().len(), 1);
        assert_eq!(buf.cursor(), (0, 0));
    }

    #[test]
    fn from_text_puts_cursor_at_end() {
        let buf = Buffer::from_text("un\ndeux");
        assert_eq!(buf.cursor(), (1, 4));
        assert_eq!(buf.text(), "un\ndeux");
    }

    #[test]
    fn round_trips_text() {
        for text in ["", "a", "a\nb", "\n", "a\n\nb", "fin\n"] {
            assert_eq!(Buffer::from_text(text).text(), text, "pour {text:?}");
        }
    }

    #[test]
    fn inserts_multibyte_chars_correctly() {
        let mut buf = Buffer::from_text("éé");
        buf.left();
        buf.insert_char('à');
        assert_eq!(buf.text(), "éàé");
    }

    #[test]
    fn backspace_removes_multibyte_char_not_byte() {
        let mut buf = Buffer::from_text("café");
        buf.backspace();
        assert_eq!(buf.text(), "caf");
    }

    #[test]
    fn backspace_at_line_start_joins_previous_line() {
        let mut buf = Buffer::from_text("un\ndeux");
        buf.home();
        buf.backspace();
        assert_eq!(buf.text(), "undeux");
        assert_eq!(buf.cursor(), (0, 2));
    }

    #[test]
    fn backspace_at_origin_is_a_noop() {
        let mut buf = Buffer::from_text("a");
        buf.up(usize::MAX);
        buf.home();
        buf.backspace();
        assert_eq!(buf.text(), "a");
    }

    #[test]
    fn delete_at_line_end_joins_next_line() {
        let mut buf = Buffer::from_text("un\ndeux");
        buf.up(usize::MAX);
        buf.home();
        buf.end();
        buf.delete();
        assert_eq!(buf.text(), "undeux");
    }

    #[test]
    fn delete_at_buffer_end_is_a_noop() {
        let mut buf = Buffer::from_text("a");
        buf.delete();
        assert_eq!(buf.text(), "a");
    }

    #[test]
    fn newline_splits_the_line_at_the_cursor() {
        let mut buf = Buffer::from_text("undeux");
        buf.home();
        buf.right();
        buf.right();
        buf.insert_newline();
        assert_eq!(buf.text(), "un\ndeux");
        assert_eq!(buf.cursor(), (1, 0));
    }

    #[test]
    fn paste_single_line_inserts_at_cursor() {
        let mut buf = Buffer::from_text("ab");
        buf.left();
        buf.insert_str("XY");
        assert_eq!(buf.text(), "aXYb");
        assert_eq!(buf.cursor(), (0, 3));
    }

    #[test]
    fn paste_multiline_splits_and_lands_cursor_after_it() {
        let mut buf = Buffer::from_text("ab");
        buf.left();
        buf.insert_str("X\nY");
        assert_eq!(buf.text(), "aX\nYb");
        assert_eq!(buf.cursor(), (1, 1));
    }

    #[test]
    fn paste_normalises_crlf_and_bare_cr() {
        let mut buf = Buffer::default();
        buf.insert_str("a\r\nb\rc");
        assert_eq!(buf.text(), "a\nb\nc");
    }

    #[test]
    fn paste_of_empty_string_changes_nothing() {
        let mut buf = Buffer::from_text("a");
        buf.insert_str("");
        assert_eq!(buf.text(), "a");
        assert_eq!(buf.cursor(), (0, 1));
    }

    #[test]
    fn vertical_move_clamps_column_to_shorter_line() {
        let mut buf = Buffer::from_text("long\nx");
        buf.up(1);
        buf.end();
        buf.down(1);
        assert_eq!(buf.cursor(), (1, 1));
    }

    #[test]
    fn vertical_moves_saturate_at_the_edges() {
        let mut buf = Buffer::from_text("a\nb\nc");
        buf.up(999);
        assert_eq!(buf.cursor().0, 0);
        buf.down(999);
        assert_eq!(buf.cursor().0, 2);
    }

    #[test]
    fn left_at_line_start_wraps_to_previous_line_end() {
        let mut buf = Buffer::from_text("un\ndeux");
        buf.home();
        buf.left();
        assert_eq!(buf.cursor(), (0, 2));
    }

    #[test]
    fn right_at_line_end_wraps_to_next_line_start() {
        let mut buf = Buffer::from_text("un\ndeux");
        buf.up(usize::MAX);
        buf.home();
        buf.end();
        buf.right();
        assert_eq!(buf.cursor(), (1, 0));
    }

    #[test]
    fn reload_clamps_cursor_when_text_shrinks() {
        let mut buf = Buffer::from_text("un\ndeux\ntrois");
        buf.replace_preserving_cursor("court");
        let (row, col) = buf.cursor();
        assert_eq!(row, 0);
        assert!(col <= 5);
    }
}
