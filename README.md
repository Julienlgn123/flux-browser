<img src="assets/logo.svg" width="96" alt="Flux">

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
- `--process-per-site` (moins de processus), pas de réseau en arrière-plan.
- V8 réglé pour la mémoire (`--js-flags=--optimize-for-size`) : YouTube passe de ~404 à ~343 Mo (jeu de travail privé, comme le Gestionnaire des tâches).
- Mode éco : mode « appareil modeste » du moteur, 3 processus de rendu au plus, pas de cache précédent/suivant : ~290 Mo sur YouTube.
- Fenêtre réduite : tous les onglets passent en mémoire réduite.
- Compteur mémoire en direct (processus Flux + WebView2) dans la barre.

## Recherche rapide
- Autocomplétion du domaine directement dans la barre, depuis l'historique (score fréquence × récence).
- Suggestions en ligne via une connexion keep-alive unique, frappe la plus récente seulement.
- Bangs : `!yt`, `!w`, `!gh`, `!g`, `!d`, `!b`, `!a`, `!r`, `!m`, `!i`, `!tr`, `!so`, `!npm`, `!rs` (avant ou après la requête).
- Moteur au choix : Google, DuckDuckGo, Brave, Bing.

## Barre de recherche rapide
- **Ctrl + Shift + X** depuis n'importe quelle application : une barre s'ouvre en haut de l'écran, Entrée lance la recherche dans Flux.
- Flux reste dans la zone de notification quand on ferme la fenêtre et démarre avec Windows, en arrière-plan : le moteur s'arrête alors complètement (~15 Mo au total). La barre rapide s'ouvre en ~0,5 s et sa vue est libérée dès qu'elle se ferme.
- Instance unique : relancer Flux réaffiche la fenêtre existante.

## Consommation en temps réel et limiteurs
- Moniteur (clic sur le compteur de la barre) : mémoire, processeur, réseau et requêtes bloquées en direct, par onglet et par processus.
- Limite de mémoire (décharge les onglets inactifs au-delà), plafond processeur matériel (Job Object Windows), débit réseau maximal par onglet.

## Comptes, chiffrement, vie privée
- Connexion persistante à Gmail, YouTube, Twitch… ; mots de passe et formulaires enregistrés par le moteur (chiffrés via DPAPI).
- Historique et session chiffrés sur le disque (DPAPI + entropie propre à Flux).
- DNS-over-HTTPS (Cloudflare par défaut), HTTPS automatique, protection anti-pistage réglable.
- Refus automatique des bannières de cookies (Google, YouTube, Didomi, OneTrust, Cookiebot…) + liste EasyList Cookie.

## Page d'accueil, favoris et historique
- Historique (Ctrl+H ou icône horloge) : regroupé par jour, recherche, suppression d'une entrée ou de tout.
- Page d'accueil épurée : logo et grande barre de recherche, sur un fond au choix (image, GIF ou vidéo locale, lue par morceaux et en pause dès qu'elle n'est pas visible), avec assombrissement réglable.
- Favoris : étoile dans la barre d'adresse ou Ctrl+D, barre de favoris, gestion (renommer, réordonner, supprimer), prioritaires dans les suggestions ; stockés chiffrés.

## Infos du site (cadenas de la barre d'adresse ou Ctrl+I)
- Connexion : IP du serveur, port, protocole (HTTP/2, HTTP/3), version TLS, chiffrement, code HTTP.
- Hébergement : opérateur / AS, ville, pays (via ipinfo.io, uniquement à l'ouverture du panneau).
- Certificat : émis pour, émetteur, validité, jours restants, domaines couverts, Certificate Transparency.
- Technologies : serveur, CDN, frameworks, CMS, outils de mesure et de pistage détectés.
- Analyse de sécurité **passive** (aucun test intrusif) avec score : HTTPS, HSTS, CSP, anti-clickjacking, nosniff, version serveur exposée, TLS obsolète, certificat bientôt expiré, contenu mixte, formulaires en HTTP, jQuery vulnérable, scripts tiers sans SRI, cookies sans Secure/HttpOnly.
- Cookies de la page (y compris tiers) : suppression un par un, ou tout effacer pour le site (cookies + stockage).
- Domaines contactés (requêtes, bloquées) avec « Refuser » les cookies par domaine : l'en-tête Cookie n'est plus envoyé et les cookies posés sont supprimés en continu.
- Données stockées : localStorage, sessionStorage, service worker, cadres intégrés.

## Personnalisation
Thème clair/sombre/système, couleur d'accent, densité, éléments de la barre, fond de la page nouvel onglet, zoom par défaut, moteur de recherche (dont personnalisé), bangs personnels, raccourci global, page de démarrage, et feuille de style CSS libre pour toute l'interface.

## Raccourcis
Ctrl+T / Ctrl+W / Ctrl+Maj+T, Ctrl+L / Alt+D / F6, Ctrl+Tab, Ctrl+1…9, F5, F12, Alt+←/→, Alt+Entrée (ouvrir dans un onglet), Maj+Suppr (retirer une entrée d'historique).

## Compiler
```
cargo build --release
```
Binaire : `target/release/flux.exe` (installé dans `%LOCALAPPDATA%\Programs\Flux`). Données : `%LOCALAPPDATA%\Flux`.
