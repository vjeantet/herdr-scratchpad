//! Décisions de lancement du pane, en fonctions pures.
//!
//! Le script lanceur ne fait qu'exécuter des commandes herdr ; **toute** la
//! logique vit ici, en modes stdin→stdout (`--launch-decision`,
//! `--focused-pane`, `--open-plan`), ce qui la rend testable sans terminal.
//!
//! Le toggle est scopé à la **tab**, comme le reste du plugin : un scratchpad
//! par tab, plusieurs tabs peuvent en avoir un, et chacun a son propre texte
//! (§9 du DESIGN). Il n'y a nulle part de clé de workspace.

use serde_json::Value;

use crate::ipc::{METADATA_SOURCE, PANE_LABEL};

/// Au-delà de ce délai sans estampille, un pane est considéré mort.
///
/// La TUI ré-estampille toutes les 5 s : la marge couvre largement une machine
/// chargée.
pub const HEARTBEAT_STALE_SECS: u64 = 20;

/// Hauteur visée pour le pane, en lignes.
///
/// Docké en bas, la ressource rare est la **hauteur**. Douze lignes laissent
/// une dizaine de lignes de texte plus la barre de boutons — assez pour un
/// outil de transit, assez peu pour ne pas amputer le terminal du dessus.
const TARGET_ROWS: f64 = 12.0;

/// Part minimale et maximale du pane scratchpad.
///
/// herdr applique déjà un plancher de 0.1 côté serveur ; ces bornes-ci sont
/// plus serrées et servent surtout aux fenêtres extrêmes (un terminal de
/// 20 lignes ne doit pas donner la moitié de l'écran au scratchpad).
const MIN_SHARE: f64 = 0.15;
const MAX_SHARE: f64 = 0.50;

/// PowerShell 5.1 préfixe un BOM UTF-8 en redirigeant vers stdin ; serde_json
/// refuse un BOM avant `{`. Sans coût sur unix.
fn strip_bom(input: &str) -> &str {
    input.trim_start_matches('\u{feff}')
}

/// Un identifiant utilisable comme argument de commande.
///
/// Un id vide, commençant par `-` (il serait pris pour un drapeau) ou porteur
/// de caractères exotiques fait dégrader la décision vers `OPEN` plutôt que de
/// construire une commande douteuse.
fn is_flag_safe(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('-')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '.' | '_' | '-'))
}

fn str_field<'a>(pane: &'a Value, key: &str) -> Option<&'a str> {
    pane.get(key)?.as_str()
}

/// Un pane est un scratchpad s'il porte notre jeton, ou à défaut notre
/// étiquette.
///
/// L'étiquette **sans** jeton est exactement la signature d'un cadavre de
/// redémarrage : herdr restaure l'étiquette et le scrollback d'un pane, mais
/// ni son processus ni ses jetons. Le reconnaître quand même permet de le
/// remplacer au lieu d'en empiler un second à côté.
fn is_scratchpad(pane: &Value) -> bool {
    let has_token = pane
        .get("tokens")
        .and_then(Value::as_object)
        .is_some_and(|t| t.contains_key(METADATA_SOURCE));
    has_token || str_field(pane, "label") == Some(PANE_LABEL)
}

/// Vrai si l'estampille sous `key` est **absente**, illisible, ou plus vieille
/// que [`HEARTBEAT_STALE_SECS`].
///
/// « Absente = mort » est sûr parce qu'un pane créé par le lanceur n'est jamais
/// observable dans cet état : le `--stamp` est synchrone et tombe entre le
/// `pane split` et le `pane run`, donc avant même que la TUI démarre. Sans
/// cette précaution, un pane lent à démarrer serait remplacé en boucle.
fn token_stale(pane: &Value, now: u64) -> bool {
    let Some(value) = pane
        .get("tokens")
        .and_then(Value::as_object)
        .and_then(|t| t.get(METADATA_SOURCE))
    else {
        return true;
    };
    let ts = value
        .as_u64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()));
    match ts {
        Some(ts) => now.saturating_sub(ts) > HEARTBEAT_STALE_SECS,
        None => true,
    }
}

fn panes(json: &str) -> Option<Vec<Value>> {
    let msg: Value = serde_json::from_str(strip_bom(json)).ok()?;
    Some(msg.get("result")?.get("panes")?.as_array()?.clone())
}

/// `OPEN`, `FOCUS <id>`, `CLOSE <id>` ou `REPLACE <id>`, à partir du JSON de
/// `herdr pane list`.
///
/// Toute entrée illisible, l'absence de pane focalisé ou un id douteux
/// dégradent vers `OPEN` : ouvrir un pane de trop est un désagrément, se
/// tromper de pane à fermer est une perte.
pub fn launch_decision(pane_list_json: &str, now: u64) -> String {
    let Some(panes) = panes(pane_list_json) else {
        return "OPEN".into();
    };
    let Some(focused) = panes
        .iter()
        .find(|p| p.get("focused").and_then(Value::as_bool) == Some(true))
    else {
        return "OPEN".into();
    };
    let focused_tab = str_field(focused, "tab_id");

    // Le toggle est scopé à la tab : un scratchpad ailleurs ne compte pas,
    // sinon `prefix+a` téléporterait vers un autre workspace.
    let Some(candidate) = panes
        .iter()
        .find(|p| is_scratchpad(p) && str_field(p, "tab_id") == focused_tab)
    else {
        return "OPEN".into();
    };
    let Some(id) = str_field(candidate, "pane_id").filter(|id| is_flag_safe(id)) else {
        return "OPEN".into();
    };

    if token_stale(candidate, now) {
        return format!("REPLACE {id}");
    }
    if str_field(focused, "pane_id") == Some(id) {
        format!("CLOSE {id}")
    } else {
        format!("FOCUS {id}")
    }
}

/// `<pane_id>\t<cwd>` du pane focalisé, ou une chaîne vide.
///
/// Le `cwd` est repassé au nouveau pane pour qu'il démarre là où l'on
/// travaille plutôt qu'au hasard.
pub fn focused_pane(pane_list_json: &str) -> String {
    let Some(panes) = panes(pane_list_json) else {
        return String::new();
    };
    let Some(focused) = panes
        .iter()
        .find(|p| p.get("focused").and_then(Value::as_bool) == Some(true))
    else {
        return String::new();
    };
    let Some(id) = str_field(focused, "pane_id").filter(|id| is_flag_safe(id)) else {
        return String::new();
    };
    format!("{id}\t{}", str_field(focused, "cwd").unwrap_or_default())
}

/// `<pane_cible>\t<ratio>` pour un dock **en bas**, à partir du JSON de
/// `herdr pane layout`.
///
/// Deux subtilités :
///
/// - on vise le pane le **plus bas** de la tab, car `pane split --direction
///   down` place le nouveau pane sous sa cible : scinder le plus bas le pose
///   donc contre le bord inférieur, sans avoir à échanger quoi que ce soit ;
/// - le `--ratio` exprime la part de la **cible**, pas celle du nouveau pane.
///   On rend donc `1 - part_voulue`.
pub fn open_plan(layout_json: &str) -> String {
    let Ok(msg) = serde_json::from_str::<Value>(strip_bom(layout_json)) else {
        return String::new();
    };
    let Some(panes) = msg
        .get("result")
        .and_then(|r| r.get("layout"))
        .and_then(|l| l.get("panes"))
        .and_then(Value::as_array)
    else {
        return String::new();
    };

    let mut best: Option<(i64, i64, String, i64)> = None;
    for pane in panes {
        let Some(id) = str_field(pane, "pane_id").filter(|id| is_flag_safe(id)) else {
            continue;
        };
        let Some(rect) = pane.get("rect") else { continue };
        let (x, y, height) = (
            rect.get("x").and_then(Value::as_i64).unwrap_or(0),
            rect.get("y").and_then(Value::as_i64).unwrap_or(0),
            rect.get("height").and_then(Value::as_i64).unwrap_or(0),
        );
        if height <= 0 {
            continue;
        }
        // Le plus bas d'abord ; à égalité, le plus à gauche.
        let key = (y + height, -x);
        if best.as_ref().is_none_or(|(by, bx, _, _)| key > (*by, *bx)) {
            best = Some((key.0, key.1, id.to_string(), height));
        }
    }

    let Some((_, _, id, height)) = best else {
        return String::new();
    };
    let share = (TARGET_ROWS / height as f64).clamp(MIN_SHARE, MAX_SHARE);
    format!("{id}\t{:.2}", 1.0 - share)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fabrique un JSON de `pane list`.
    fn list(panes: &str) -> String {
        format!(r#"{{"id":"x","result":{{"panes":[{panes}]}}}}"#)
    }

    fn pane(id: &str, tab: &str, focused: bool, token: Option<&str>) -> String {
        let tokens = match token {
            Some(ts) => format!(r#","tokens":{{"herdr-scratchpad":"{ts}"}}"#),
            None => String::new(),
        };
        format!(
            r#"{{"pane_id":"{id}","tab_id":"{tab}","cwd":"/tmp","focused":{focused}{tokens}}}"#
        )
    }

    #[test]
    fn garbage_input_degrades_to_open() {
        assert_eq!(launch_decision("pas du json", 100), "OPEN");
        assert_eq!(launch_decision("", 100), "OPEN");
        assert_eq!(launch_decision(r#"{"result":{}}"#, 100), "OPEN");
    }

    #[test]
    fn no_focused_pane_degrades_to_open() {
        let json = list(&pane("w1:p1", "w1:t1", false, Some("100")));
        assert_eq!(launch_decision(&json, 100), "OPEN");
    }

    #[test]
    fn no_scratchpad_in_the_tab_opens() {
        let json = list(&pane("w1:p1", "w1:t1", true, None));
        assert_eq!(launch_decision(&json, 100), "OPEN");
    }

    #[test]
    fn focused_scratchpad_closes() {
        let json = list(&pane("w1:p2", "w1:t1", true, Some("100")));
        assert_eq!(launch_decision(&json, 100), "CLOSE w1:p2");
    }

    #[test]
    fn unfocused_scratchpad_in_same_tab_focuses() {
        let json = list(&format!(
            "{},{}",
            pane("w1:p1", "w1:t1", true, None),
            pane("w1:p2", "w1:t1", false, Some("100"))
        ));
        assert_eq!(launch_decision(&json, 100), "FOCUS w1:p2");
    }

    /// Le cœur du scope-à-la-tab : un scratchpad ailleurs ne doit pas empêcher
    /// d'en ouvrir un ici, sinon `prefix+a` téléporterait l'utilisateur.
    #[test]
    fn scratchpad_in_another_tab_is_ignored() {
        let json = list(&format!(
            "{},{}",
            pane("w1:p1", "w1:t1", true, None),
            pane("w2:p9", "w2:t1", false, Some("100"))
        ));
        assert_eq!(launch_decision(&json, 100), "OPEN");
    }

    #[test]
    fn stale_token_is_replaced() {
        let json = list(&format!(
            "{},{}",
            pane("w1:p1", "w1:t1", true, None),
            pane("w1:p2", "w1:t1", false, Some("10"))
        ));
        assert_eq!(
            launch_decision(&json, 10 + HEARTBEAT_STALE_SECS + 1),
            "REPLACE w1:p2"
        );
    }

    #[test]
    fn token_exactly_at_the_threshold_is_still_alive() {
        let json = list(&format!(
            "{},{}",
            pane("w1:p1", "w1:t1", true, None),
            pane("w1:p2", "w1:t1", false, Some("10"))
        ));
        assert_eq!(
            launch_decision(&json, 10 + HEARTBEAT_STALE_SECS),
            "FOCUS w1:p2"
        );
    }

    /// Un cadavre de redémarrage : étiquette restaurée, jeton perdu.
    #[test]
    fn labelled_pane_without_token_is_a_corpse() {
        let json = list(&format!(
            r#"{},{{"pane_id":"w1:p2","tab_id":"w1:t1","focused":false,"label":"{PANE_LABEL}"}}"#,
            pane("w1:p1", "w1:t1", true, None)
        ));
        assert_eq!(launch_decision(&json, 100), "REPLACE w1:p2");
    }

    /// Un cadavre focalisé se ranime au lieu de se fermer : fermer ne
    /// laisserait rien derrière, et le geste de l'utilisateur était « montre-moi
    /// le scratchpad ».
    #[test]
    fn focused_corpse_is_replaced_not_closed() {
        let json = list(&format!(
            r#"{{"pane_id":"w1:p2","tab_id":"w1:t1","focused":true,"label":"{PANE_LABEL}"}}"#
        ));
        assert_eq!(launch_decision(&json, 100), "REPLACE w1:p2");
    }

    #[test]
    fn unsafe_pane_id_degrades_to_open() {
        let json = list(&format!(
            "{},{}",
            pane("w1:p1", "w1:t1", true, None),
            pane("--rm -rf", "w1:t1", false, Some("100"))
        ));
        assert_eq!(launch_decision(&json, 100), "OPEN");
    }

    #[test]
    fn unparsable_token_counts_as_dead() {
        let json = list(
            r#"{"pane_id":"w1:p2","tab_id":"w1:t1","focused":false,"tokens":{"herdr-scratchpad":{}}},{"pane_id":"w1:p1","tab_id":"w1:t1","focused":true}"#,
        );
        assert_eq!(launch_decision(&json, 100), "REPLACE w1:p2");
    }

    #[test]
    fn focused_pane_reports_id_and_cwd() {
        let json = list(&pane("w1:p1", "w1:t1", true, None));
        assert_eq!(focused_pane(&json), "w1:p1\t/tmp");
    }

    #[test]
    fn focused_pane_is_empty_without_a_focused_one() {
        let json = list(&pane("w1:p1", "w1:t1", false, None));
        assert_eq!(focused_pane(&json), "");
    }

    fn layout(panes: &str) -> String {
        format!(r#"{{"result":{{"layout":{{"panes":[{panes}]}}}}}}"#)
    }

    fn lpane(id: &str, x: i64, y: i64, w: i64, h: i64) -> String {
        format!(
            r#"{{"pane_id":"{id}","rect":{{"x":{x},"y":{y},"width":{w},"height":{h}}}}}"#
        )
    }

    #[test]
    fn open_plan_targets_the_bottom_most_pane() {
        let json = layout(&format!(
            "{},{}",
            lpane("top", 0, 0, 80, 20),
            lpane("bottom", 0, 20, 80, 20)
        ));
        assert!(open_plan(&json).starts_with("bottom\t"));
    }

    /// 12 lignes sur 40 font une part de 0.30 pour le scratchpad, donc 0.70
    /// pour la cible — le `--ratio` est bien celui de la cible.
    #[test]
    fn open_plan_ratio_is_the_targets_share() {
        let json = layout(&lpane("only", 0, 0, 80, 40));
        assert_eq!(open_plan(&json), "only\t0.70");
    }

    #[test]
    fn open_plan_clamps_share_on_a_tall_window() {
        // 12/200 = 0.06, sous le plancher : la part monte à 0.15.
        let json = layout(&lpane("only", 0, 0, 80, 200));
        assert_eq!(open_plan(&json), "only\t0.85");
    }

    #[test]
    fn open_plan_clamps_share_on_a_short_window() {
        // 12/16 = 0.75, au-dessus du plafond : la part retombe à 0.50.
        let json = layout(&lpane("only", 0, 0, 80, 16));
        assert_eq!(open_plan(&json), "only\t0.50");
    }

    #[test]
    fn open_plan_skips_zero_height_and_unsafe_panes() {
        let json = layout(&format!(
            "{},{}",
            lpane("good", 0, 0, 80, 40),
            lpane("--bad", 0, 40, 80, 40)
        ));
        assert!(open_plan(&json).starts_with("good\t"));

        let json = layout(&lpane("flat", 0, 0, 80, 0));
        assert_eq!(open_plan(&json), "");
    }

    #[test]
    fn open_plan_of_garbage_is_empty() {
        assert_eq!(open_plan("nope"), "");
        assert_eq!(open_plan(r#"{"result":{}}"#), "");
    }
}
