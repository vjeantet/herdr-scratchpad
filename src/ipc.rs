//! Client minimal de l'API socket de herdr.
//!
//! Sert uniquement à estampiller le pane (« je suis vivant »), ce qui permet
//! au lanceur de distinguer un pane scratchpad vivant d'un cadavre laissé par
//! un redémarrage du serveur.
//!
//! Protocole : JSON délimité par des sauts de ligne, **une requête par
//! connexion**. Toutes les erreurs sont avalées : hors herdr, le pane doit
//! rester parfaitement utilisable.

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

/// Envoie une requête et ignore la réponse.
fn call(method: &str, params: serde_json::Value) -> Option<()> {
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
        // On lit la réponse pour ne pas laisser herdr écrire dans un tuyau
        // fermé, mais son contenu ne nous intéresse pas.
        let mut line = String::new();
        let _ = BufReader::new(&stream).read_line(&mut line);
        Some(())
    }
    #[cfg(not(unix))]
    {
        let _ = request;
        None
    }
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

    let _ = call(
        "pane.report_metadata",
        serde_json::json!({
            "pane_id": pane_id,
            "source": METADATA_SOURCE,
            "title": PANE_LABEL,
            "tokens": { METADATA_SOURCE: now.to_string() },
        }),
    );
}
