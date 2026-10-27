// Refus automatique des bannières de cookies (injecté dans chaque page et cadre).
// 1. API des plateformes de consentement courantes (Didomi, OneTrust, Cookiebot, Usercentrics, tarteaucitron).
// 2. Sinon, clic sur le bouton « Tout refuser » — uniquement à l'intérieur d'une bannière de
//    consentement, pour ne jamais cliquer sur un autre bouton « Refuser » (invitation, ami…).
(() => {
  const host = location.hostname;
  if (/^(mail|calendar|chat|meet|drive|docs)\.google\./.test(host)) return;
  // Libellés « tout refuser » sans ambiguïté : acceptés partout dans une page de consentement.
  const STRICT = /^\s*(tout refuser|refuser tout|tout rejeter|rejeter tout|refuser tous les cookies|reject all|reject all cookies|decline all|refuse all|deny all|alle ablehnen|rechazar todo|rifiuta tutto|alles weigeren)\s*$/i;
  // Libellés plus courts : seulement à l'intérieur d'une bannière de cookies identifiée.
  const LOOSE = /^\s*(refuser|refuser et fermer|continuer sans accepter|reject|decline|deny|only necessary( cookies)?|necessary only|use necessary cookies only|accept only necessary|nécessaires uniquement|cookies nécessaires uniquement|ablehnen|rechazar|rifiuta|weigeren)\s*$/i;
  const CTX = /cookie|consent|gdpr|rgpd|\bcmp|didomi|onetrust|privacy|qc-cmp|sp_message|truste|axeptio|tarteaucitron|cookiebot|usercentrics|cky-|osano|iubenda|klaro|sourcepoint|fc-consent/i;
  const consentPage = /(^|\.)consent\./.test(host) || /^(www\.)?(google|youtube)\./.test(host);

  const inBanner = el => {
    for (let n = el, i = 0; n && i < 14; n = n.parentElement, i++) {
      const sig = (n.id || '') + ' ' + (typeof n.className === 'string' ? n.className : '') + ' ' + (n.getAttribute && (n.getAttribute('aria-label') || '') || '');
      if (CTX.test(sig)) return true;
    }
    return false;
  };
  const visible = el => !!(el.offsetWidth || el.offsetHeight || el.getClientRects().length);

  let done = false, apisDone = false;
  function viaApi() {
    if (apisDone) return;
    const w = window;
    try { if (w.Didomi && w.Didomi.setUserDisagreeToAll) { w.Didomi.setUserDisagreeToAll(); apisDone = true; } } catch {}
    try { if (w.OneTrust && w.OneTrust.RejectAll) { w.OneTrust.RejectAll(); apisDone = true; } } catch {}
    try { if (w.Cookiebot && w.Cookiebot.submitCustomConsent) { w.Cookiebot.submitCustomConsent(false, false, false); apisDone = true; } } catch {}
    try { if (w.UC_UI && w.UC_UI.denyAllConsents) { w.UC_UI.denyAllConsents(); apisDone = true; } } catch {}
    try { if (w.tarteaucitron && w.tarteaucitron.userInterface) { w.tarteaucitron.userInterface.respondAll(false); apisDone = true; } } catch {}
  }
  function viaButton() {
    for (const el of document.querySelectorAll('button, [role=button], input[type=submit], input[type=button], a')) {
      const label = (el.innerText || el.value || el.getAttribute('aria-label') || '').trim();
      if (!label || label.length > 45) continue;
      const strict = STRICT.test(label);
      if (!strict && !LOOSE.test(label)) continue;
      if (!(strict && consentPage) && !inBanner(el)) continue;
      if (!visible(el)) continue;
      el.click();
      return true;
    }
    return false;
  }
  function run() {
    if (done) return;
    viaApi();
    if (viaButton()) done = true;
  }
  function start() {
    run();
    let pending = false;
    const mo = new MutationObserver(() => {
      if (done || pending) return;
      pending = true;
      setTimeout(() => { pending = false; run(); }, 250);
    });
    mo.observe(document.documentElement, { childList: true, subtree: true });
    // Les bannières apparaissent dans les premières secondes : inutile de surveiller au-delà.
    setTimeout(() => mo.disconnect(), 15000);
  }
  document.readyState === 'loading' ? addEventListener('DOMContentLoaded', start, { once: true }) : start();
})();
