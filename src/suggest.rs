//! Suggestions de recherche distantes. Un seul thread, une seule connexion
//! keep-alive réutilisée : la latence se limite à un aller-retour HTTP.

use std::{
    sync::mpsc::{channel, Sender},
    thread,
    time::Duration,
};

pub struct Request {
    pub gen: u64,
    pub query: String,
    pub engine: String,
}

pub fn spawn(reply: impl Fn(u64, Vec<String>) + Send + 'static) -> Sender<Request> {
    let (tx, rx) = channel::<Request>();
    thread::spawn(move || {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_millis(1500)))
            .build()
            .into();
        while let Ok(mut req) = rx.recv() {
            // Ne traiter que la frappe la plus récente.
            while let Ok(newer) = rx.try_recv() {
                req = newer;
            }
            let items = fetch(&agent, &req.engine, &req.query).unwrap_or_default();
            reply(req.gen, items);
        }
    });
    tx
}

fn fetch(agent: &ureq::Agent, engine: &str, q: &str) -> Option<Vec<String>> {
    // Tous ces points d'accès renvoient le format OpenSearch : ["q", ["s1", "s2", ...]]
    let (url, key) = match engine {
        "duckduckgo" => ("https://duckduckgo.com/ac/", "q"),
        "brave" => ("https://search.brave.com/api/suggest", "q"),
        "bing" => ("https://api.bing.com/osjson.aspx", "query"),
        _ => ("https://suggestqueries.google.com/complete/search", "q"),
    };
    let mut req = agent.get(url).query(key, q);
    if engine == "duckduckgo" {
        req = req.query("type", "list");
    } else if !matches!(engine, "brave" | "bing") {
        req = req.query("client", "firefox");
    }
    let body = req.call().ok()?.body_mut().read_to_string().ok()?;
    let v: serde_json::Value = serde_json::from_str(&body).ok()?;
    Some(
        v.get(1)?
            .as_array()?
            .iter()
            .filter_map(|s| s.as_str().map(str::to_owned))
            .take(6)
            .collect(),
    )
}
