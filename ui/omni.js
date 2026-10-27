// Logique de recherche partagée par la barre d'adresse et la barre de recherche rapide.
// Chaque page déclare les globales `settings` (réglages), `hist` (Map url → visite) et `favs` (favoris).
const ENGINES = {
  google: 'https://www.google.com/search?q=%s', duckduckgo: 'https://duckduckgo.com/?q=%s',
  brave: 'https://search.brave.com/search?q=%s', bing: 'https://www.bing.com/search?q=%s',
  ecosia: 'https://www.ecosia.org/search?q=%s', qwant: 'https://www.qwant.com/?q=%s',
};
const BANGS = {
  g: ENGINES.google, d: ENGINES.duckduckgo, b: ENGINES.brave,
  yt: 'https://www.youtube.com/results?search_query=%s', w: 'https://fr.wikipedia.org/w/index.php?search=%s',
  gh: 'https://github.com/search?q=%s', a: 'https://www.amazon.fr/s?k=%s', r: 'https://www.reddit.com/search/?q=%s',
  m: 'https://www.google.com/maps/search/%s', i: 'https://www.google.com/search?udm=2&q=%s',
  tr: 'https://translate.google.com/?sl=auto&tl=fr&text=%s', so: 'https://stackoverflow.com/search?q=%s',
  npm: 'https://www.npmjs.com/search?q=%s', rs: 'https://docs.rs/releases/search?query=%s',
  tw: 'https://www.twitch.tv/search?term=%s', gm: 'https://mail.google.com/mail/u/0/#search/%s',
};
const ICON_SEARCH = '<svg viewBox="0 0 24 24"><circle cx="11" cy="11" r="7"/><path d="M20 20l-3.5-3.5"/></svg>';
const ICON_GO = '<svg viewBox="0 0 24 24"><path d="M5 12h14M13 6l6 6-6 6"/></svg>';

const esc = s => String(s).replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
const bare = u => u.replace(/^https?:\/\/(www\.)?/i, '').replace(/\/$/, '');
const host = u => { try { return new URL(u).hostname.replace(/^www\./, ''); } catch { return ''; } };
const frecency = v => v.visits / (1 + (Date.now() / 1000 - v.last) / 86400 / 7);

/** Bangs intégrés + bangs personnels des réglages (« clé=https://…%s », un par ligne). */
function allBangs() {
  const out = { ...BANGS };
  for (const line of (settings.bangs || '').split('\n')) {
    const m = line.trim().match(/^!?(\w+)\s*=\s*(\S+)$/);
    if (m) out[m[1]] = m[2].includes('%s') ? m[2] : m[2] + '%s';
  }
  return out;
}
const fill = (tpl, q) => tpl.replace('%s', encodeURIComponent(q));
function engineUrl() {
  if (settings.engine === 'custom' && (settings.custom_engine || '').includes('%s')) return settings.custom_engine;
  return ENGINES[settings.engine] || ENGINES.google;
}

function looksUrl(s) {
  if (/\s/.test(s)) return false;
  if (/^[a-z][\w+.-]*:\/\//i.test(s) || /^(about|data|file|edge|view-source):/i.test(s)) return true;
  if (/^localhost(:\d+)?(\/|$)/i.test(s) || /^\d{1,3}(\.\d{1,3}){3}(:\d+)?(\/|$)/.test(s)) return true;
  return /^[^\s\/:]+\.[a-z]{2,}(:\d+)?(\/.*)?$/i.test(s);
}
function bangOf(s) {
  // « !yt chats » ou « chats !yt »
  const pre = s.match(/^!(\w+)\s+(.+)$/), post = s.match(/^(.+?)\s+!(\w+)$/);
  const [k, q] = pre ? [pre[1], pre[2]] : post ? [post[2], post[1]] : [];
  const tpl = k && allBangs()[k];
  return tpl ? { tpl, q } : null;
}
function toUrl(s) {
  s = s.trim();
  const bang = bangOf(s);
  if (bang) return fill(bang.tpl, bang.q);
  if (looksUrl(s)) {
    if (/^[a-z][\w+.-]*:/i.test(s) && !/^localhost:/i.test(s)) return s;
    return (/^(localhost|\d{1,3}(\.\d{1,3}){3})/i.test(s) ? 'http://' : 'https://') + s;
  }
  return fill(engineUrl(), s);
}

const favFailed = new Set();
function fav(url) {
  const h = host(url), letter = esc((h[0] || '•').toUpperCase());
  if (!h || favFailed.has(h)) return `<span class="fav">${letter}</span>`;
  let origin; try { origin = new URL(url).origin; } catch { return `<span class="fav">${letter}</span>`; }
  return `<span class="fav"><img src="${esc(origin)}/favicon.ico" data-h="${esc(h)}" data-l="${letter}" onerror="favErr(this)"></span>`;
}
function favErr(img) { favFailed.add(img.dataset.h); img.parentNode.textContent = img.dataset.l; }

function localMatches(q, limit = 5) {
  const words = q.toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return [];
  const ql = q.toLowerCase(), out = [];
  for (const v of hist.values()) {
    const b = bare(v.url).toLowerCase(), hay = b + ' ' + v.title.toLowerCase();
    if (!words.every(w => hay.includes(w))) continue;
    let s = frecency(v);
    if (b.startsWith(ql)) s *= 6; else if (host(v.url).includes(ql)) s *= 2;
    out.push([s, v]);
  }
  return out.sort((a, b) => b[0] - a[0]).slice(0, limit).map(x => x[1]);
}

// Complète le domaine le plus probable directement dans le champ (comme Chrome).
function inlineComplete(q) {
  if (!q || /\s/.test(q)) return null;
  const ql = q.toLowerCase();
  let best = null, bs = 0;
  for (const v of hist.values()) {
    const b = bare(v.url).toLowerCase();
    if (!b.startsWith(ql)) continue;
    const s = frecency(v);
    if (s > bs) { bs = s; best = b; }
  }
  if (!best) return null;
  const slash = best.indexOf('/', ql.length);
  const done = slash === -1 ? best : best.slice(0, Math.max(slash, ql.length));
  return done.length > ql.length ? done : null;
}

/** Lignes de suggestions pour une saisie : aller à / rechercher / historique / suggestions en ligne. */
function suggestionRows(typed, completed, remote, max = 10) {
  const q = typed.trim(), rows = [];
  if (!q) return rows;
  if (completed) rows.push({ icon: 'go', main: completed, url: toUrl(completed) });
  else if (looksUrl(q)) rows.push({ icon: 'go', main: q, url: toUrl(q) });
  const bang = bangOf(q);
  rows.push({ icon: 'search', main: q, sub: bang ? host(fill(bang.tpl, 'x')) : 'Rechercher', url: toUrl(q) });
  const seen = new Set(rows.map(r => r.url));
  const words = q.toLowerCase().split(/\s+/).filter(Boolean);
  for (const f of favs) {
    if (rows.length >= 5) break;
    const hay = (bare(f.url) + ' ' + f.title).toLowerCase();
    if (!words.every(w => hay.includes(w)) || seen.has(f.url)) continue;
    seen.add(f.url);
    rows.push({ icon: 'hist', fav: f.url, main: f.title || bare(f.url), sub: '★ ' + bare(f.url), url: f.url });
  }
  for (const v of localMatches(q)) {
    if (seen.has(v.url)) continue; seen.add(v.url);
    rows.push({ icon: 'hist', fav: v.url, main: v.title || bare(v.url), sub: bare(v.url), url: v.url });
  }
  for (const s of remote) {
    if (rows.length >= max) break;
    if (s.toLowerCase() === q.toLowerCase()) continue;
    const url = toUrl(s);
    if (seen.has(url)) continue; seen.add(url);
    rows.push({ icon: 'search', main: s, url });
  }
  return rows;
}
function rowHtml(r, i, sel) {
  return `<div class="row${i === sel ? ' sel' : ''}" data-i="${i}">${r.icon === 'hist' ? fav(r.fav) : r.icon === 'go' ? ICON_GO : ICON_SEARCH}` +
    `<span class="main">${esc(r.main)}</span>${r.sub ? `<span class="sub">${esc(r.sub)}</span>` : ''}${i === sel ? '<kbd>Entrée</kbd>' : ''}</div>`;
}
const wantsRemote = q => settings.suggestions && q && !looksUrl(q) && !q.startsWith('!');

/** Thème et couleur d'accent communs aux deux interfaces. */
function applyTheme(s) {
  const dark = s.theme === 'dark' || (s.theme === 'system' && matchMedia('(prefers-color-scheme: dark)').matches);
  document.documentElement.dataset.theme = dark ? 'dark' : 'light';
  document.documentElement.style.setProperty('--accent', s.accent || '#ff5b2e');
  document.documentElement.style.setProperty('--accent-bg', (s.accent || '#ff5b2e') + '22');
}
