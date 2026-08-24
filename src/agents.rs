//! Les agents joignables, et le choix de celui vers qui envoyer.
//!
//! Tout est **pur** : ces fonctions prennent le JSON de `agent.list` et de
//! `workspace.list` et rendent des cibles. Aucun socket, aucun disque — même
//! discipline que `launch.rs`, pour la même raison : un bug de sélection de
//! cible doit se reproduire avec une chaîne de caractères, pas en ouvrant des
//! panes à la main.

use serde_json::Value;

/// Ce qu'affiche la zone cible quand il n'y a personne à qui envoyer.
pub const NO_TARGET: &str = "→ aucun agent";

/// Nom de repli quand herdr ne nomme pas l'agent.
///
/// `agent` est optionnel dans `AgentInfo` : laisser tomber l'entrée priverait
/// l'utilisateur d'une cible parfaitement valide pour une étiquette manquante.
const UNNAMED: &str = "agent";

/// Un agent joignable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub pane_id: String,
    /// Nom court de l'agent, `claude` par exemple.
    pub agent: String,
    pub tab_id: String,
    pub workspace_id: String,
    /// Libellé du workspace, ou son id quand il n'en a pas.
    pub workspace_label: String,
}

/// Où vit **ce** pane, tel que herdr le lui a dit.
///
/// Les deux viennent de l'environnement (`HERDR_TAB_ID`, `HERDR_WORKSPACE_ID`),
/// que herdr injecte dans tout pane qu'il crée — y compris ceux nés d'un
/// `pane split`, vérifié.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Home {
    pub tab_id: Option<String>,
    pub workspace_id: Option<String>,
}

fn str_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str().filter(|s| !s.is_empty())
}

fn array<'a>(json: &'a Value, key: &str) -> Option<&'a Vec<Value>> {
    json.get("result")?.get(key)?.as_array()
}

/// Croise `agent.list` et `workspace.list` en cibles ordonnées.
///
/// `exclude_pane` retire le pane courant : le scratchpad n'est pas un agent et
/// ne devrait jamais apparaître, mais s'envoyer son propre texte serait un
/// piège assez déroutant pour valoir la ligne de garde.
///
/// Du JSON illisible rend une liste vide : le pane doit rester utilisable même
/// quand herdr répond n'importe quoi.
pub fn targets(agents_json: &str, workspaces_json: &str, exclude_pane: Option<&str>) -> Vec<Target> {
    let labels: Vec<(String, String)> = serde_json::from_str::<Value>(workspaces_json)
        .ok()
        .as_ref()
        .and_then(|json| array(json, "workspaces"))
        .map(|list| {
            list.iter()
                .filter_map(|w| {
                    Some((
                        str_field(w, "workspace_id")?.to_owned(),
                        str_field(w, "label")?.to_owned(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();

    let Ok(json) = serde_json::from_str::<Value>(agents_json) else {
        return Vec::new();
    };
    let Some(list) = array(&json, "agents") else {
        return Vec::new();
    };

    let mut out: Vec<Target> = list
        .iter()
        .filter_map(|a| {
            let pane_id = str_field(a, "pane_id")?.to_owned();
            if Some(pane_id.as_str()) == exclude_pane {
                return None;
            }
            let agent = str_field(a, "agent")
                .or_else(|| str_field(a, "display_agent"))
                .or_else(|| str_field(a, "name"))
                .unwrap_or(UNNAMED)
                .to_owned();
            let tab_id = str_field(a, "tab_id").unwrap_or_default().to_owned();
            let workspace_id = str_field(a, "workspace_id").unwrap_or_default().to_owned();
            let workspace_label = labels
                .iter()
                .find(|(id, _)| *id == workspace_id)
                .map(|(_, label)| label.clone())
                // Un workspace sans libellé retombe sur son id : mieux vaut
                // une cible moche qu'une cible anonyme.
                .unwrap_or_else(|| workspace_id.clone());

            Some(Target { pane_id, agent, tab_id, workspace_id, workspace_label })
        })
        .collect();

    // L'ordre doit être **stable** d'un rafraîchissement à l'autre : herdr ne
    // garantit pas l'ordre de `agent.list`, et un cyclage qui saute d'un
    // rafraîchissement à l'autre est pire que pas de cyclage du tout.
    out.sort_by(|a, b| {
        a.workspace_id
            .cmp(&b.workspace_id)
            .then_with(|| a.pane_id.cmp(&b.pane_id))
    });
    out
}

/// Applique l'ordre de préférence : **tab courante**, puis workspace courant,
/// puis dernière cible mémorisée, puis la première venue.
///
/// La tab passe avant le workspace parce que c'est elle qui désigne « l'agent
/// du pane courant » : le scratchpad naît d'un split du pane focalisé, donc
/// dans **sa** tab. Un workspace, lui, peut porter plusieurs tabs et donc
/// plusieurs agents sans rapport avec celui qu'on regardait.
///
/// `remembered` est une paire *(libellé de workspace, agent)* et non un
/// `pane_id` : un `pane_id` ne survit pas à un redémarrage de herdr, alors que
/// cette paire se retrouve.
pub fn pick_default(
    targets: &[Target],
    home: &Home,
    remembered: Option<(&str, &str)>,
) -> Option<usize> {
    if targets.is_empty() {
        return None;
    }
    if let Some(tab) = home.tab_id.as_deref().filter(|t| !t.is_empty())
        && let Some(i) = targets.iter().position(|t| t.tab_id == tab)
    {
        return Some(i);
    }
    if let Some(workspace) = home.workspace_id.as_deref().filter(|w| !w.is_empty())
        && let Some(i) = targets.iter().position(|t| t.workspace_id == workspace)
    {
        return Some(i);
    }
    if let Some((label, agent)) = remembered
        && let Some(i) = targets
            .iter()
            .position(|t| t.workspace_label == label && t.agent == agent)
    {
        return Some(i);
    }
    Some(0)
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

/// Libellé de la zone cible, rogné à `width` colonnes.
///
/// Le workspace tombe **entier** avant que le nom de l'agent ne soit entamé :
/// un `→ claude·herdr-scr` tronqué désignerait aussi bien un autre workspace
/// commençant pareil, ce qui est exactement le contraire du garde-fou
/// recherché.
pub fn label(target: Option<&Target>, width: usize) -> String {
    let Some(target) = target else {
        return truncate(NO_TARGET, width);
    };
    let full = format!("→ {}·{}", target.agent, target.workspace_label);
    if columns(&full) <= width {
        return full;
    }
    truncate(&format!("→ {}", target.agent), width)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Une réponse `agent.list` réduite à ce qu'on en lit. La tab est déduite
    /// du `pane_id` (`w1:p1` -> `w1:t1`), comme le fait herdr par défaut.
    fn agents(entries: &[(&str, &str, &str)]) -> String {
        let list: Vec<Value> = entries
            .iter()
            .map(|(pane_id, agent, workspace_id)| {
                serde_json::json!({
                    "pane_id": pane_id,
                    "agent": agent,
                    "workspace_id": workspace_id,
                    "tab_id": format!("{workspace_id}:t1"),
                    "focused": false,
                })
            })
            .collect();
        serde_json::json!({ "id": "x", "result": { "type": "agent_list", "agents": list } })
            .to_string()
    }

    fn workspaces(entries: &[(&str, &str)]) -> String {
        let list: Vec<Value> = entries
            .iter()
            .map(|(id, label)| serde_json::json!({ "workspace_id": id, "label": label }))
            .collect();
        serde_json::json!({ "id": "x", "result": { "type": "workspace_list", "workspaces": list } })
            .to_string()
    }

    fn target(agent: &str, label: &str) -> Target {
        tabbed(agent, label, &format!("{label}:t1"))
    }

    fn tabbed(agent: &str, label: &str, tab_id: &str) -> Target {
        Target {
            pane_id: format!("{label}:{agent}"),
            agent: agent.into(),
            tab_id: tab_id.into(),
            workspace_id: label.into(),
            workspace_label: label.into(),
        }
    }

    fn home(tab: Option<&str>, workspace: Option<&str>) -> Home {
        Home {
            tab_id: tab.map(str::to_owned),
            workspace_id: workspace.map(str::to_owned),
        }
    }

    #[test]
    fn une_liste_vide_ne_rend_aucune_cible() {
        assert!(targets(&agents(&[]), &workspaces(&[]), None).is_empty());
    }

    #[test]
    fn un_agent_est_croise_avec_le_libelle_de_son_workspace() {
        let found = targets(
            &agents(&[("w2:p1", "claude", "w2")]),
            &workspaces(&[("w2", "wdv")]),
            None,
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].agent, "claude");
        assert_eq!(found[0].workspace_label, "wdv");
    }

    #[test]
    fn la_tab_de_l_agent_est_lue() {
        let found = targets(&agents(&[("w2:p1", "claude", "w2")]), &workspaces(&[]), None);
        assert_eq!(found[0].tab_id, "w2:t1");
    }

    #[test]
    fn un_workspace_sans_libelle_retombe_sur_son_id() {
        let found = targets(&agents(&[("w9:p1", "claude", "w9")]), &workspaces(&[]), None);
        assert_eq!(found[0].workspace_label, "w9");
    }

    #[test]
    fn un_agent_sans_nom_recoit_un_nom_de_repli() {
        let json = r#"{"result":{"agents":[{"pane_id":"w1:p1","workspace_id":"w1"}]}}"#;
        let found = targets(json, &workspaces(&[]), None);
        assert_eq!(found[0].agent, UNNAMED);
    }

    #[test]
    fn le_pane_courant_est_exclu() {
        let found = targets(
            &agents(&[("w1:p1", "claude", "w1"), ("w1:p2", "claude", "w1")]),
            &workspaces(&[]),
            Some("w1:p1"),
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].pane_id, "w1:p2");
    }

    /// L'ordre de `agent.list` n'est pas garanti : deux entrées inversées
    /// doivent rendre la même liste, sinon le cyclage saute.
    #[test]
    fn l_ordre_est_stable_quel_que_soit_celui_de_la_reponse() {
        let ws = workspaces(&[("w1", "un"), ("w2", "deux")]);
        let a = targets(&agents(&[("w2:p1", "claude", "w2"), ("w1:p1", "codex", "w1")]), &ws, None);
        let b = targets(&agents(&[("w1:p1", "codex", "w1"), ("w2:p1", "claude", "w2")]), &ws, None);
        assert_eq!(a, b);
        assert_eq!(a[0].pane_id, "w1:p1");
    }

    #[test]
    fn du_json_illisible_ne_panique_pas_et_rend_une_liste_vide() {
        assert!(targets("pas du json", "pas du json non plus", None).is_empty());
        assert!(targets(r#"{"result":{}}"#, r#"{"result":{}}"#, None).is_empty());
    }

    /// Un `workspace.list` cassé ne doit pas priver d'agents : les libellés
    /// retombent sur les ids.
    #[test]
    fn des_workspaces_illisibles_laissent_les_agents_utilisables() {
        let found = targets(&agents(&[("w1:p1", "claude", "w1")]), "cassé", None);
        assert_eq!(found[0].workspace_label, "w1");
    }

    /// La préférence n° 1 : « l'agent du pane courant », c'est-à-dire celui de
    /// la tab où le scratchpad vient de naître.
    #[test]
    fn pick_default_prefere_l_agent_de_la_tab_courante() {
        let list = [
            tabbed("claude", "w3", "w3:t1"),
            tabbed("claude", "w3", "w3:t2"),
        ];
        assert_eq!(pick_default(&list, &home(Some("w3:t2"), Some("w3")), None), Some(1));
    }

    /// Le cas qui a motivé la règle : deux agents dans le même workspace mais
    /// dans deux tabs. Sans la tab, c'est le voisin qui sortait.
    #[test]
    fn la_tab_l_emporte_sur_le_workspace() {
        let list = [
            tabbed("claude", "w3", "w3:t1"),
            tabbed("claude", "w3", "w3:t2"),
        ];
        let sans_tab = pick_default(&list, &home(None, Some("w3")), None);
        let avec_tab = pick_default(&list, &home(Some("w3:t2"), Some("w3")), None);
        assert_eq!(sans_tab, Some(0));
        assert_eq!(avec_tab, Some(1), "la tab doit désigner l'agent d'à côté");
    }

    /// Un scratchpad seul dans sa tab n'a pas d'agent chez lui : il retombe
    /// sur le workspace plutôt que sur n'importe qui.
    #[test]
    fn sans_agent_dans_la_tab_on_retombe_sur_le_workspace() {
        let list = [target("claude", "w1"), target("codex", "w2")];
        assert_eq!(
            pick_default(&list, &home(Some("w2:t9"), Some("w2")), None),
            Some(1)
        );
    }

    #[test]
    fn pick_default_retrouve_la_cible_memorisee() {
        let list = [target("claude", "w1"), target("codex", "w2")];
        assert_eq!(
            pick_default(&list, &Home::default(), Some(("w2", "codex"))),
            Some(1)
        );
    }

    /// La proximité l'emporte : la mémoire sert à retrouver une cible quand le
    /// pane n'a pas d'agent chez lui, pas à la contredire.
    #[test]
    fn la_proximite_l_emporte_sur_la_memoire() {
        let list = [target("claude", "w1"), target("codex", "w2")];
        assert_eq!(
            pick_default(&list, &home(Some("w1:t1"), None), Some(("w2", "codex"))),
            Some(0)
        );
        assert_eq!(
            pick_default(&list, &home(None, Some("w1")), Some(("w2", "codex"))),
            Some(0)
        );
    }

    #[test]
    fn pick_default_prend_le_premier_a_defaut() {
        let list = [target("claude", "w1"), target("codex", "w2")];
        assert_eq!(
            pick_default(
                &list,
                &home(Some("inconnue"), Some("inconnu")),
                Some(("absent", "absent"))
            ),
            Some(0)
        );
    }

    #[test]
    fn pick_default_rend_rien_sur_une_liste_vide() {
        assert_eq!(pick_default(&[], &home(Some("w1:t1"), Some("w1")), None), None);
    }

    #[test]
    fn next_boucle_et_revient_au_debut() {
        let list = [target("a", "w1"), target("b", "w2")];
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
        let list = [target("a", "w1")];
        assert_eq!(next(&list, None), Some(0));
    }

    #[test]
    fn label_affiche_l_agent_et_son_workspace() {
        assert_eq!(label(Some(&target("claude", "wdv")), 40), "→ claude·wdv");
    }

    #[test]
    fn label_rogne_le_workspace_avant_le_nom_de_l_agent() {
        let t = target("claude", "herdr-scratchpad");
        assert_eq!(label(Some(&t), 15), "→ claude");
    }

    #[test]
    fn label_sans_cible_dit_qu_il_n_y_en_a_aucune() {
        assert_eq!(label(None, 40), NO_TARGET);
    }

    #[test]
    fn label_ne_deborde_jamais_de_la_largeur() {
        let t = target("claude", "wdv");
        for width in 0..20 {
            assert!(columns(&label(Some(&t), width)) <= width, "largeur {width}");
            assert!(columns(&label(None, width)) <= width, "largeur {width}");
        }
    }
}
