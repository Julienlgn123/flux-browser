// Analyse passive de la page ouverte, pour « Infos du site » : lit le document,
// ne modifie rien et n'envoie aucune requête.
(() => {
  const w = window, d = document, tech = [];
  const add = (name, cat, ver) => { if (!tech.some(t => t.name === name)) tech.push({ name, cat, ver: ver ? String(ver) : '' }); };
  const has = sel => { try { return !!d.querySelector(sel); } catch { return false; } };
  const src = re => [...d.scripts].some(s => re.test(s.src));

  const gen = d.querySelector('meta[name="generator" i]');
  if (gen && gen.content) add(gen.content.split(/[ ;]/)[0], 'CMS / générateur', (gen.content.match(/\d+(\.\d+)+/) || [])[0]);
  if (w.React || has('[data-reactroot]')) add('React', 'Framework JavaScript', w.React && w.React.version);
  if (w.__NEXT_DATA__ || has('#__next')) add('Next.js', 'Framework web', w.next && w.next.version);
  if (w.__NUXT__ || has('#__nuxt')) add('Nuxt', 'Framework web');
  if (w.Vue || has('[data-v-app]')) add('Vue.js', 'Framework JavaScript', w.Vue && w.Vue.version);
  if (w.angular || has('[ng-version]')) add('Angular', 'Framework JavaScript', has('[ng-version]') ? d.querySelector('[ng-version]').getAttribute('ng-version') : '');
  if (has('[class*="svelte-"]')) add('Svelte', 'Framework JavaScript');
  if (w.Polymer || has('ytd-app')) add('Polymer', 'Framework JavaScript');
  if (w.Alpine) add('Alpine.js', 'Framework JavaScript', w.Alpine.version);
  if (w.htmx) add('htmx', 'Bibliothèque JavaScript', w.htmx.version);
  if (w.jQuery) add('jQuery', 'Bibliothèque JavaScript', w.jQuery.fn && w.jQuery.fn.jquery);
  if (w.bootstrap || has('link[href*="bootstrap"]')) add('Bootstrap', 'Framework CSS');
  if (has('link[href*="tailwind"]') || src(/tailwind/)) add('Tailwind CSS', 'Framework CSS');
  if (w.wp || has('link[href*="wp-content"], script[src*="wp-content"]')) add('WordPress', 'CMS');
  if (w.Shopify) add('Shopify', 'E-commerce');
  if (w.Drupal) add('Drupal', 'CMS');
  if (w.Joomla) add('Joomla', 'CMS');
  if (w.Webflow) add('Webflow', 'Créateur de site');
  if (w.wixBiSession || has('meta[name="generator"][content*="Wix"]')) add('Wix', 'Créateur de site');
  if (w.Squarespace) add('Squarespace', 'Créateur de site');
  if (w.PrestaShop || w.prestashop) add('PrestaShop', 'E-commerce');
  if (w.Stripe || src(/js\.stripe\.com/)) add('Stripe', 'Paiement');
  if (w.paypal || src(/paypal\.com\/sdk/)) add('PayPal', 'Paiement');
  if (w.gtag || w.ga || src(/google-analytics\.com|googletagmanager\.com\/gtag/)) add('Google Analytics', 'Mesure d\'audience');
  if (w.google_tag_manager || src(/googletagmanager\.com\/gtm/)) add('Google Tag Manager', 'Gestion de balises');
  if (w.fbq) add('Meta Pixel', 'Publicité / pistage');
  if (w.ttq) add('TikTok Pixel', 'Publicité / pistage');
  if (w.hj) add('Hotjar', 'Enregistrement de sessions');
  if (w.clarity) add('Microsoft Clarity', 'Enregistrement de sessions');
  if (w._paq) add('Matomo', 'Mesure d\'audience');
  if (w.plausible) add('Plausible', 'Mesure d\'audience');
  if (w.Sentry || w.__SENTRY__) add('Sentry', 'Suivi d\'erreurs');
  if (w.Intercom) add('Intercom', 'Support client');
  if (w.Didomi) add('Didomi', 'Gestion du consentement');
  if (w.OneTrust) add('OneTrust', 'Gestion du consentement');
  if (w.Cookiebot) add('Cookiebot', 'Gestion du consentement');
  if (w.grecaptcha) add('reCAPTCHA', 'Anti-robot');
  if (w.hcaptcha) add('hCaptcha', 'Anti-robot');
  if (w.turnstile) add('Cloudflare Turnstile', 'Anti-robot');

  const insecure = sel => d.querySelectorAll(sel).length;
  const mixed = location.protocol === 'https:'
    ? insecure('img[src^="http:"], script[src^="http:"], link[rel=stylesheet][href^="http:"], iframe[src^="http:"], video[src^="http:"], audio[src^="http:"]')
    : 0;
  const forms = [...d.forms];
  const httpForms = forms.filter(f => /^http:/i.test(f.action)).length;
  const passwordOnHttp = location.protocol === 'http:' && has('input[type=password]');
  const hosts = new Set(), noSri = new Set();
  for (const s of d.scripts) {
    if (!s.src) continue;
    try {
      const h = new URL(s.src).hostname;
      if (h !== location.hostname) { hosts.add(h); if (!s.integrity) noSri.add(h); }
    } catch {}
  }
  const size = st => { try { let n = 0; for (let i = 0; i < st.length; i++) { const k = st.key(i); n += k.length + (st.getItem(k) || '').length; } return [st.length, n * 2]; } catch { return [0, 0]; } };
  const [lsKeys, lsBytes] = size(w.localStorage), [ssKeys, ssBytes] = size(w.sessionStorage);

  return {
    title: d.title,
    tech,
    mixed,
    httpForms,
    passwordOnHttp,
    passwordFields: insecure('input[type=password]'),
    thirdPartyScripts: [...hosts],
    noSri: [...noSri],
    unsafeBlank: insecure('a[target=_blank]:not([rel~=noopener]):not([rel~=noreferrer])'),
    iframes: insecure('iframe'),
    localStorage: { keys: lsKeys, bytes: lsBytes },
    sessionStorage: { keys: ssKeys, bytes: ssBytes },
    serviceWorker: !!(navigator.serviceWorker && navigator.serviceWorker.controller),
  };
})()
