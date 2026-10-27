# Flux

Navigateur Windows ultra-léger basé sur Chromium (moteur WebView2 d'Edge, déjà installé sur Windows).
Exécutable de ~2,3 Mo, écrit en Rust (tao + wry).

## Performance
- **Onglets paresseux** : à la restauration de session, seul l'onglet actif charge sa page.
- **Gestion mémoire automatique** des onglets cachés : mémoire réduite après 30 s, gel complet après 5 min, déchargement après 60 min (réglable). Les onglets qui jouent du son sont épargnés.
- **Bloqueur pubs/traqueurs** filtré côté moteur (WebView2) : seules les requêtes bloquées remontent à l'application.
- `--process-per-site` (moins de processus), pas de réseau en arrière-plan, mode éco optionnel (3 processus de rendu max).
- Compteur mémoire en direct (processus Flux + WebView2) dans la barre.

## Recherche rapide
- Autocomplétion du domaine directement dans la barre, depuis l'historique (score fréquence × récence).
- Suggestions en ligne via une connexion keep-alive unique, frappe la plus récente seulement.
- Bangs : `!yt`, `!w`, `!gh`, `!g`, `!d`, `!b`, `!a`, `!r`, `!m`, `!i`, `!tr`, `!so`, `!npm`, `!rs` (avant ou après la requête).
- Moteur au choix : Google, DuckDuckGo, Brave, Bing.

## Raccourcis
Ctrl+T / Ctrl+W / Ctrl+Maj+T, Ctrl+L / Alt+D / F6, Ctrl+Tab, Ctrl+1…9, F5, F12, Alt+←/→, Alt+Entrée (ouvrir dans un onglet), Maj+Suppr (retirer une entrée d'historique).

## Compiler
```
cargo build --release
```
Binaire : `target/release/flux.exe`. Données : `%LOCALAPPDATA%\Flux`.
