//! Copie vers le presse-papier de l'hôte via OSC 52.
//!
//! La machine cible est typiquement headless et atteinte en SSH : il n'y a ni
//! X, ni Wayland, ni `xclip`. OSC 52 est donc le seul chemin — et le bon, car
//! il dépose le texte dans le presse-papier de la machine *où l'on va coller*.
//!
//! herdr parse la séquence émise par l'enfant du pane (`src/pane/osc.rs`), la
//! relaie à travers le serveur (`src/protocol/wire.rs:701`) et préfère déjà
//! OSC 52 aux outils natifs quand il détecte SSH (`src/selection.rs`).

use std::io::{self, Write};

/// Plafond appliqué par herdr aux écritures presse-papier
/// (`MAX_CLIPBOARD_BYTES`, `src/ghostty/mod.rs:468`).
///
/// Le test porte sur les octets **décodés** (`src/ghostty/mod.rs:612`), donc
/// sur la taille du texte, pas celle du base64.
pub const MAX_CLIPBOARD_BYTES: usize = 192 * 1024;

/// Pourquoi une copie n'a pas eu lieu.
#[derive(Debug, PartialEq, Eq)]
pub enum CopyError {
    /// Buffer vide. herdr rejette les payloads vides (`UNSUPPORTED`), donc on
    /// ne tente même pas.
    Empty,
    /// Au-delà de `MAX_CLIPBOARD_BYTES` herdr jette silencieusement l'écriture.
    /// On refuse en amont pour pouvoir renvoyer l'utilisateur vers l'export
    /// plutôt que de le laisser coller du vide.
    TooLarge { len: usize },
    /// L'écriture sur la sortie a échoué.
    Io(String),
}

/// Encode `input` en base64 standard, avec padding.
///
/// Réimplémenté plutôt que tiré d'une dépendance : trente lignes contre un
/// crate de plus dans un binaire qui n'en a que trois.
pub fn base64(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;

        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Construit la séquence OSC 52.
///
/// Terminateur **BEL** et non ST : certains terminaux n'honorent que celui-là,
/// et c'est la forme que herdr émet lui-même (`src/selection.rs:352`).
fn osc52_sequence(bytes: &[u8]) -> String {
    format!("\x1b]52;c;{}\x07", base64(bytes))
}

/// Vérifie qu'un texte est copiable avant de tenter quoi que ce soit.
pub fn check(text: &str) -> Result<(), CopyError> {
    if text.is_empty() {
        return Err(CopyError::Empty);
    }
    if text.len() > MAX_CLIPBOARD_BYTES {
        return Err(CopyError::TooLarge { len: text.len() });
    }
    Ok(())
}

/// Copie `text` vers le presse-papier de l'hôte.
///
/// Écrit directement sur la sortie : la séquence est hors-bande, elle traverse
/// le rendu ratatui sans l'abîmer.
pub fn copy(text: &str) -> Result<usize, CopyError> {
    check(text)?;

    let mut out = io::stdout();
    out.write_all(osc52_sequence(text.as_bytes()).as_bytes())
        .and_then(|()| out.flush())
        .map_err(|e| CopyError::Io(e.to_string()))?;
    Ok(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64_handles_non_utf8_and_high_bytes() {
        assert_eq!(base64(&[0xff, 0xfe, 0xfd]), "//79");
        assert_eq!(base64(&[0x00, 0x00, 0x00]), "AAAA");
    }

    /// La forme exacte attendue par herdr, reprise de son propre test
    /// (`src/selection.rs:352`).
    #[test]
    fn sequence_matches_herdr_own_test_vector() {
        assert_eq!(osc52_sequence(b"hello"), "\x1b]52;c;aGVsbG8=\x07");
    }

    #[test]
    fn empty_is_refused_because_herdr_rejects_empty_payloads() {
        assert_eq!(check(""), Err(CopyError::Empty));
    }

    #[test]
    fn limit_is_inclusive() {
        let exact = "x".repeat(MAX_CLIPBOARD_BYTES);
        assert_eq!(check(&exact), Ok(()));

        let over = "x".repeat(MAX_CLIPBOARD_BYTES + 1);
        assert_eq!(
            check(&over),
            Err(CopyError::TooLarge {
                len: MAX_CLIPBOARD_BYTES + 1
            })
        );
    }

    /// Le plafond de herdr porte sur les octets décodés : un texte de
    /// multi-octets doit être mesuré en octets, pas en caractères.
    #[test]
    fn limit_counts_bytes_not_chars() {
        let text = "é".repeat(MAX_CLIPBOARD_BYTES / 2 + 1); // 2 octets chacun
        assert!(matches!(check(&text), Err(CopyError::TooLarge { .. })));
    }
}
