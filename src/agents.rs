//! Les agents joignables, et le choix de celui vers qui envoyer.
//!
//! Tout est **pur** : ces fonctions prennent le JSON de `agent.list` et de
//! `tab.list` et rendent des cibles. Aucun socket, aucun disque — même
//! discipline que `launch.rs`, pour la même raison : un bug de sélection de
//! cible doit se reproduire avec une chaîne de caractères, pas en ouvrant des
//! panes à la main.
//!
//! La portée est la **tab** : les agents d'ailleurs ne sont pas des cibles,
//! quoi qu'il arrive (§14.4 du DESIGN). Un scratchpad seul dans sa tab n'a
//! donc aucune cible — c'est un bloc-notes local, et c'est assumé.

use serde_json::Value;

/// Nom de repli quand herdr ne nomme pas l'agent.
///
/// `agent` est optionnel dans `AgentInfo` : laisser tomber l'entrée priverait
/// l'utilisateur d'une cible parfaitement valide pour une étiquette manquante.
const UNNAMED: &str = "agent";

/// Un agent joignable, dans **ma** tab.
///
/// Ni `tab_id` ni `workspace_id` : ils sont constants par construction, tous
/// les habitants de cette liste vivant dans la tab de ce pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub pane_id: String,
    /// Nom court de l'agent, `claude` par exemple.
    pub agent: String,
}

fn str_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str().filter(|s| !s.is_empty())
}

fn array<'a>(json: &'a Value, key: &str) -> Option<&'a Vec<Value>> {
    json.get("result")?.get(key)?.as_array()
}

/// Les agents de `tab_id`, ordonnés par `pane_id`.
///
/// `agent.list` porte déjà le `tab_id` de chaque agent : le filtre est une
/// comparaison de chaînes, sans appel supplémentaire.
///
/// `tab_id` à `None` — hors herdr — rend une liste **vide** : pas de tab, pas
/// de cible. Un binaire lancé à la main ne doit pas pouvoir déposer du texte
/// quelque part.
///
/// `exclude_pane` retire le pane courant : le scratchpad n'est pas un agent et
/// ne devrait jamais apparaître, mais s'envoyer son propre texte serait un
/// piège assez déroutant pour valoir la ligne de garde.
///
/// Du JSON illisible rend une liste vide : le pane doit rester utilisable même
/// quand herdr répond n'importe quoi.
pub fn targets(agents_json: &str, tab_id: Option<&str>, exclude_pane: Option<&str>) -> Vec<Target> {
    let Some(tab_id) = tab_id.filter(|t| !t.is_empty()) else {
        return Vec::new();
    };
    let Ok(json) = serde_json::from_str::<Value>(agents_json) else {
        return Vec::new();
    };
    let Some(list) = array(&json, "agents") else {
        return Vec::new();
    };

    let mut out: Vec<Target> = list
        .iter()
        .filter_map(|a| {
            if str_field(a, "tab_id") != Some(tab_id) {
                return None;
            }
            let pane_id = str_field(a, "pane_id")?.to_owned();
            if Some(pane_id.as_str()) == exclude_pane {
                return None;
            }
            let agent = str_field(a, "agent")
                .or_else(|| str_field(a, "display_agent"))
                .or_else(|| str_field(a, "name"))
                .unwrap_or(UNNAMED)
                .to_owned();

            Some(Target { pane_id, agent })
        })
        .collect();

    // L'ordre doit être **stable** d'un rafraîchissement à l'autre : herdr ne
    // garantit pas l'ordre de `agent.list`, et un cyclage qui saute d'un
    // rafraîchissement à l'autre est pire que pas de cyclage du tout.
    out.sort_by(|a, b| a.pane_id.cmp(&b.pane_id));
    out
}

/// Les `tab_id` vivants d'une réponse `tab.list`.
///
/// Du JSON illisible rend une liste vide, ce qui déclenche l'abstention du
/// ménage par construction : une liste vide n'est jamais une information, et
/// l'appelant refuse de supprimer quoi que ce soit sur cette base.
pub fn live_tab_ids(tabs_json: &str) -> Vec<String> {
    let Ok(json) = serde_json::from_str::<Value>(tabs_json) else {
        return Vec::new();
    };
    let Some(list) = array(&json, "tabs") else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|t| Some(str_field(t, "tab_id")?.to_owned()))
        .collect()
}

/// Cible suivante, en boucle.
pub fn next(targets: &[Target], current: Option<usize>) -> Option<usize> {
    if targets.is_empty() {
        return None;
    }
    match current {
        // Un index périmé (la liste a rétréci) retombe sur le début plutôt que
        // de rendre `None` : le cyclage ne doit jamais se bloquer.
        Some(i) if i + 1 < targets.len() => Some(i + 1),
        Some(_) => Some(0),
        None => Some(0),
    }
}

fn columns(s: &str) -> usize {
    use unicode_width::UnicodeWidthChar;
    s.chars().map(|c| UnicodeWidthChar::width(c).unwrap_or(0)).sum()
}

/// Tronque à `width` colonnes d'affichage.
fn truncate(s: &str, width: usize) -> String {
    if columns(s) <= width {
        return s.to_owned();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for c in s.chars() {
        use unicode_width::UnicodeWidthChar;
        let w = UnicodeWidthChar::width(c).unwrap_or(0);
        if used + w > width {
            break;
        }
        used += w;
        out.push(c);
    }
    out
}

/// Le discriminant d'une cible : la part du `pane_id` après le dernier `:`.
///
/// C'est le seul à la fois unique, stable et vérifiable au `herdr pane list`.
/// Le nom de l'agent, lui, ne distingue rien : deux `claude` dans la même tab
/// portent le même.
fn suffix(pane_id: &str) -> &str {
    pane_id.rsplit(':').next().unwrap_or(pane_id)
}

/// Libellé de la zone cible, rogné à `width` colonnes : `→ claude·p3`.
///
/// Le suffixe tombe **entier** avant que le nom de l'agent ne soit entamé : un
/// suffixe tronqué désignerait aussi bien le pane d'à côté, ce qui est
/// exactement le contraire du garde-fou recherché.
pub fn label(target: &Target, width: usize) -> String {
    let full = format!("→ {}·{}", target.agent, suffix(&target.pane_id));
    if columns(&full) <= width {
        return full;
    }
    truncate(&format!("→ {}", target.agent), width)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Une réponse `agent.list` réduite à ce qu'on en lit.
    fn agents(entries: &[(&str, &str, &str)]) -> String {
        let list: Vec<Value> = entries
            .iter()
            .map(|(pane_id, agent, tab_id)| {
                serde_json::json!({
                    "pane_id": pane_id,
                    "agent": agent,
                    "tab_id": tab_id,
                    "workspace_id": tab_id.split(':').next().unwrap_or(""),
                    "focused": false,
                })
            })
            .collect();
        serde_json::json!({ "id": "x", "result": { "type": "agent_list", "agents": list } })
            .to_string()
    }

    fn tabs(ids: &[&str]) -> String {
        let list: Vec<Value> = ids
            .iter()
            .map(|id| serde_json::json!({ "tab_id": id, "workspace_id": "w1", "number": 1 }))
            .collect();
        serde_json::json!({ "id": "x", "result": { "type": "tab_list", "tabs": list } }).to_string()
    }

    fn target(agent: &str, pane_id: &str) -> Target {
        Target { pane_id: pane_id.into(), agent: agent.into() }
    }

    #[test]
    fn une_liste_vide_ne_rend_aucune_cible() {
        assert!(targets(&agents(&[]), Some("w1:t1"), None).is_empty());
    }

    #[test]
    fn un_agent_de_ma_tab_est_une_cible() {
        let found = targets(&agents(&[("w2:p1", "claude", "w2:t1")]), Some("w2:t1"), None);
        assert_eq!(found, vec![target("claude", "w2:p1")]);
    }

    /// Le cœur du cloisonnement : ce qui vit ailleurs n'est pas joignable,
    /// quoi qu'il arrive.
    #[test]
    fn un_agent_d_une_autre_tab_n_est_pas_une_cible() {
        let found = targets(&agents(&[("w2:p1", "claude", "w2:t9")]), Some("w2:t1"), None);
        assert!(found.is_empty(), "seule ma tab compte");
    }

    #[test]
    fn deux_agents_de_ma_tab_sont_deux_cibles_ordonnees_par_pane_id() {
        let found = targets(
            &agents(&[("w1:p2", "codex", "w1:t1"), ("w1:p1", "claude", "w1:t1")]),
            Some("w1:t1"),
            None,
        );
        assert_eq!(
            found,
            vec![target("claude", "w1:p1"), target("codex", "w1:p2")]
        );
    }

    #[test]
    fn une_tab_inconnue_ne_rend_aucune_cible() {
        let found = targets(&agents(&[("w1:p1", "claude", "w1:t1")]), Some("w9:t9"), None);
        assert!(found.is_empty());
    }

    /// Hors herdr, il n'y a pas de tab : on ne dépose nulle part.
    #[test]
    fn sans_tab_id_il_n_y_a_aucune_cible() {
        let found = targets(&agents(&[("w1:p1", "claude", "w1:t1")]), None, None);
        assert!(found.is_empty());
        assert!(targets(&agents(&[("w1:p1", "claude", "w1:t1")]), Some(""), None).is_empty());
    }

    #[test]
    fn le_pane_courant_reste_exclu_meme_dans_la_bonne_tab() {
        let found = targets(
            &agents(&[("w1:p1", "claude", "w1:t1"), ("w1:p2", "claude", "w1:t1")]),
            Some("w1:t1"),
            Some("w1:p1"),
        );
        assert_eq!(found, vec![target("claude", "w1:p2")]);
    }

    #[test]
    fn un_agent_sans_nom_recoit_un_nom_de_repli() {
        let json = r#"{"result":{"agents":[{"pane_id":"w1:p1","tab_id":"w1:t1"}]}}"#;
        let found = targets(json, Some("w1:t1"), None);
        assert_eq!(found[0].agent, UNNAMED);
    }

    /// L'ordre de `agent.list` n'est pas garanti : deux entrées inversées
    /// doivent rendre la même liste, sinon le cyclage saute.
    #[test]
    fn l_ordre_est_stable_quel_que_soit_celui_de_la_reponse() {
        let a = targets(
            &agents(&[("w1:p2", "codex", "w1:t1"), ("w1:p1", "claude", "w1:t1")]),
            Some("w1:t1"),
            None,
        );
        let b = targets(
            &agents(&[("w1:p1", "claude", "w1:t1"), ("w1:p2", "codex", "w1:t1")]),
            Some("w1:t1"),
            None,
        );
        assert_eq!(a, b);
        assert_eq!(a[0].pane_id, "w1:p1");
    }

    #[test]
    fn du_json_illisible_ne_panique_pas_et_rend_une_liste_vide() {
        assert!(targets("pas du json", Some("w1:t1"), None).is_empty());
        assert!(targets(r#"{"result":{}}"#, Some("w1:t1"), None).is_empty());
    }

    // -- tabs vivantes ----------------------------------------------------

    #[test]
    fn live_tab_ids_rend_les_ids_d_un_tab_list_bien_forme() {
        assert_eq!(
            live_tab_ids(&tabs(&["w1:t1", "w2:t3"])),
            vec!["w1:t1".to_owned(), "w2:t3".to_owned()]
        );
    }

    /// Une liste vide déclenche l'abstention du ménage : c'est une panne, pas
    /// une information.
    #[test]
    fn live_tab_ids_rend_une_liste_vide_sur_du_json_illisible() {
        assert!(live_tab_ids("pas du json").is_empty());
        assert!(live_tab_ids(r#"{"result":{}}"#).is_empty());
        assert!(live_tab_ids(r#"{"error":{"code":"nope"}}"#).is_empty());
    }

    // -- cyclage ----------------------------------------------------------

    #[test]
    fn next_boucle_et_revient_au_debut() {
        let list = [target("a", "w1:p1"), target("b", "w1:p2")];
        assert_eq!(next(&list, Some(0)), Some(1));
        assert_eq!(next(&list, Some(1)), Some(0));
    }

    #[test]
    fn next_sur_une_liste_vide_rend_rien() {
        assert_eq!(next(&[], Some(0)), None);
        assert_eq!(next(&[], None), None);
    }

    #[test]
    fn next_sans_cible_courante_prend_la_premiere() {
        let list = [target("a", "w1:p1")];
        assert_eq!(next(&list, None), Some(0));
    }

    // -- libellé ----------------------------------------------------------

    #[test]
    fn label_affiche_l_agent_et_le_suffixe_de_son_pane() {
        assert_eq!(label(&target("claude", "w2:p3"), 40), "→ claude·p3");
    }

    /// Le discriminant qui manquait : deux `claude` dans la même tab ne se
    /// lisaient pas l'un de l'autre.
    #[test]
    fn deux_agents_de_meme_nom_ont_deux_libelles_distincts() {
        assert_ne!(
            label(&target("claude", "w3:p1"), 40),
            label(&target("claude", "w3:p2"), 40)
        );
    }

    #[test]
    fn label_rogne_le_suffixe_avant_le_nom_de_l_agent() {
        assert_eq!(label(&target("claude", "w2:p3"), 9), "→ claude");
    }

    #[test]
    fn label_ne_deborde_jamais_de_la_largeur() {
        let t = target("claude", "w2:p3");
        for width in 0..20 {
            assert!(columns(&label(&t, width)) <= width, "largeur {width}");
        }
    }

    /// Un `pane_id` sans `:` n'est pas prévu par herdr, mais il ne doit pas
    /// produire de libellé vide.
    #[test]
    fn un_pane_id_sans_deux_points_sert_de_suffixe_entier() {
        assert_eq!(label(&target("claude", "p9"), 40), "→ claude·p9");
    }
}
