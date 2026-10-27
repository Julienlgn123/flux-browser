# Flux

Navigateur Windows ultra-léger basé sur Chromium (moteur WebView2 d'Edge, déjà installé sur Windows).
Exécutable de ~2,3 Mo, écrit en Rust (tao + wry).

## Performance
- **Onglets paresseux** : à la restauration de session, seul l'onglet actif charge sa page.
- **Gestion mémoire automatique** des onglets cachés : mémoire réduite après 30 s, gel complet après 5 min, déchargement après 60 min (réglable). Les onglets qui jouent du son sont épargnés.
- **Bloqueur complet EasyList / uBlock Origin** via adblock-rust (moteur de Brave) : EasyList, EasyPrivacy, Liste FR, filtres uBlock (pubs, vie privée, malveillants, correctifs, unbreak) et Peter Lowe, ~165 000 règles.
  - Filtrage réseau de chaque requête, avec redirections `$redirect` (scripts neutres) pour ne pas casser les sites.
  - Filtrage cosmétique (`##`, règles génériques par classe/id) et scriptlets uBlock (`+js(...)`).
  - Listes mises à jour automatiquement toutes les 24 h en arrière-plan ; moteur compilé mis en cache (`engine.dat`) pour un démarrage instantané.
  - Score 100/100 sur adblock-tester.com.
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
