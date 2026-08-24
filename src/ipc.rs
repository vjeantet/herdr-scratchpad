//! Client minimal de l'API socket de herdr.
//!
//! Deux usages : estampiller le pane (« je suis vivant »), ce qui permet au
//! lanceur de distinguer un pane scratchpad vivant d'un cadavre laissé par un
//! redémarrage du serveur ; et le dépôt de texte chez un agent (§14 du
//! DESIGN).
//!
//! Protocole : JSON délimité par des sauts de ligne, **une requête par
//! connexion**. Hors herdr, tout dégrade silencieusement : le pane doit rester
//! parfaitement utilisable sans serveur.

use std::io::{BufRead, BufReader, Write};

/// Nom sous lequel herdr connaît ce plugin, et clé du jeton de vivacité.
pub const METADATA_SOURCE: &str = "herdr-scratchpad";
/// Étiquette du pane. Sert aussi de reconnaissance de repli quand le jeton a
/// disparu (cf. `launch::is_scratchpad`).
pub const PANE_LABEL: &str = "Scratchpad";

fn socket_path() -> Option<String> {
    std::env::var("HERDR_SOCKET_PATH").ok().filter(|p| !p.is_empty())
}

#[cfg(unix)]
fn connect(path: &str) -> std::io::Result<std::os::unix::net::UnixStream> {
    std::os::unix::net::UnixStream::connect(path)
}

/// Envoie une requête et rend la ligne de réponse brute.
///
/// Rendre le **texte** plutôt qu'une valeur déjà parsée garde le parsing dans
/// `agents.rs`, où il est pur et testable sans socket.
fn call_raw(method: &str, params: serde_json::Value) -> Option<String> {
    let path = socket_path()?;
    let request = serde_json::json!({
        "id": format!("{METADATA_SOURCE}:{method}"),
        "method": method,
        "params": params,
    });

    #[cfg(unix)]
    {
        let mut stream = connect(&path).ok()?;
        stream.write_all(request.to_string().as_bytes()).ok()?;
        stream.write_all(b"\n").ok()?;
        stream.flush().ok()?;
        // Toujours lire la réponse, même quand on n'en fait rien : sans ça
        // herdr écrirait dans un tuyau déjà fermé.
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).ok()?;
        Some(line)
    }
    #[cfg(not(unix))]
    {
        let _ = request;
        None
    }
}

/// Le message d'erreur d'une réponse, s'il y en a une.
///
/// herdr répond `{"id":…,"error":{"code":…,"message":…}}` — un échec arrive
/// donc avec un statut de transport parfaitement normal, et se lit ici.
fn error_of(line: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let error = value.get("error")?;
    let message = error
        .get("message")
        .and_then(serde_json::Value::as_str)
        .or_else(|| error.get("code").and_then(serde_json::Value::as_str))
        .unwrap_or("erreur inconnue");
    Some(message.to_owned())
}

/// Estampille le pane avec l'instant courant.
///
/// La valeur du jeton **doit être une chaîne** : herdr rejette les nombres
/// avec `invalid_request`.
pub fn stamp(pane_id: &str) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let _ = call_raw(
        "pane.report_metadata",
        serde_json::json!({
            "pane_id": pane_id,
            "source": METADATA_SOURCE,
            "title": PANE_LABEL,
            "tokens": { METADATA_SOURCE: now.to_string() },
        }),
    );
}

/// Ce que l'application attend du serveur herdr.
///
/// L'indirection existe pour les tests : `App` en tient un exemplaire, et le
/// banc d'essai en substitue un qui répond du JSON figé. Aucun test unitaire
/// n'ouvre de socket.
pub trait Herdr {
    /// JSON brut de `agent.list`.
    fn agent_list(&self) -> Option<String>;
    /// JSON brut de `tab.list`, **non scopé** : toutes les tabs de tous les
    /// workspaces. C'est la liste des tabs vivantes, et elle ne sert qu'au
    /// ménage des buffers orphelins, au démarrage.
    fn tab_list(&self) -> Option<String>;
    /// Dépose `text` dans la boîte de saisie d'un pane, **sans le soumettre**.
    fn send_input(&self, pane_id: &str, text: &str) -> Result<(), String>;
    /// Pose le focus sur l'agent d'un pane.
    fn focus_agent(&self, pane_id: &str);
}

/// L'implémentation réelle, par le socket.
pub struct Socket;

impl Herdr for Socket {
    fn agent_list(&self) -> Option<String> {
        call_raw("agent.list", serde_json::json!({}))
    }

    /// `workspace_id` est optionnel dans `TabListParams` : sans lui, herdr
    /// parcourt tous les workspaces (`herdr src/app/api/tabs.rs:21`). Un seul
    /// appel suffit donc, et il remplace celui que `workspace.list` coûtait à
    /// chaque rafraîchissement.
    fn tab_list(&self) -> Option<String> {
        call_raw("tab.list", serde_json::json!({}))
    }

    /// `keys: []` est le cœur de la fonctionnalité : le texte atterrit dans le
    /// champ de saisie de l'agent et **on n'envoie pas Entrée**. L'utilisateur
    /// bascule, relit, soumet lui-même (§14 du DESIGN).
    ///
    /// Contrairement à [`stamp`], l'erreur remonte : c'est elle qui empêche le
    /// scratchpad de se vider sur un envoi raté.
    fn send_input(&self, pane_id: &str, text: &str) -> Result<(), String> {
        let line = call_raw(
            "pane.send_input",
            serde_json::json!({
                "pane_id": pane_id,
                "text": text,
                "keys": [],
            }),
        )
        .ok_or_else(|| "herdr injoignable".to_owned())?;

        match error_of(&line) {
            Some(message) => Err(message),
            None => Ok(()),
        }
    }

    /// `agent.focus` bascule tab **et** workspace au besoin
    /// (`herdr src/app/agents.rs:75` -> `switch_workspace_tab`). Le `pane_id`
    /// public est accepté comme cible et résolu en premier — le nom de
    /// l'agent, lui, serait ambigu entre plusieurs `claude`
    /// (`herdr src/app/terminal_targets.rs:79`).
    ///
    /// L'échec est **avalé** : le texte est déjà parti, et rater la bascule ne
    /// doit pas transformer un envoi réussi en message d'erreur.
    fn focus_agent(&self, pane_id: &str) {
        let _ = call_raw("agent.focus", serde_json::json!({ "target": pane_id }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn une_reponse_en_erreur_est_reconnue() {
        let line = r#"{"id":"x","error":{"code":"pane_not_found","message":"pas de pane"}}"#;
        assert_eq!(error_of(line).as_deref(), Some("pas de pane"));
    }

    #[test]
    fn une_reponse_reussie_ne_porte_pas_d_erreur() {
        let line = r#"{"id":"x","result":{"type":"ok"}}"#;
        assert_eq!(error_of(line), None);
    }

    #[test]
    fn une_erreur_sans_message_retombe_sur_son_code() {
        let line = r#"{"id":"x","error":{"code":"invalid_request"}}"#;
        assert_eq!(error_of(line).as_deref(), Some("invalid_request"));
    }

    /// Une ligne illisible n'est pas une erreur du serveur : ne pas la
    /// transformer en échec d'envoi, qui bloquerait le vidage à tort.
    #[test]
    fn une_ligne_illisible_ne_devient_pas_une_erreur() {
        assert_eq!(error_of("ceci n'est pas du json"), None);
    }
}
