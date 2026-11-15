// Goatr: YouTube ads. YouTube serves its video ads from its own servers, inside the same data
// as the video, so blocking addresses isn't enough (uBlock does the same as this): the ad
// entries are removed from the player's data before YouTube reads them, ad boxes are hidden,
// and any ad that still starts is skipped.
(() => {
  const h = location.hostname;
  if (!/(^|\.)youtube(-nocookie)?\.com$/.test(h)) return;
  if (window.__goatrYT) return;
  window.__goatrYT = true;

  const AD_KEYS = ['adPlacements', 'adSlots', 'playerAds', 'adBreakHeartbeatParams'];
  const hasAds = (o) => o && typeof o === 'object' && AD_KEYS.some((k) => k in o);
  const prune = (o) => {
    if (!o || typeof o !== 'object') return o;
    for (const k of AD_KEYS) if (k in o) delete o[k];
    if (o.playerResponse) prune(o.playerResponse);
    if (Array.isArray(o)) o.forEach(prune);
    return o;
  };

  // Player data parsed from JSON (page data and API answers).
  const parse = JSON.parse;
  JSON.parse = function (...args) {
    const r = parse.apply(this, args);
    try {
      if (hasAds(r) || (r && hasAds(r.playerResponse))) prune(r);
    } catch (e) {}
    return r;
  };

  // The first video's data, set by the page before the player starts.
  let initial;
  try {
    Object.defineProperty(window, 'ytInitialPlayerResponse', {
      configurable: true,
      get: () => initial,
      set: (v) => { initial = prune(v); },
    });
  } catch (e) {}

  // Later videos' data, fetched from YouTube's API.
  const json = Response.prototype.json;
  Response.prototype.json = function () {
    const url = this.url || '';
    return json.call(this).then((r) => {
      try {
        if (/\/youtubei\/v1\/(player|next|browse|search|reel)/.test(url)) prune(r);
      } catch (e) {}
      return r;
    });
  };

  // Ad boxes in the page, and the "ad blockers aren't allowed" notice.
  const CSS = [
    'ytd-ad-slot-renderer', 'ytd-in-feed-ad-layout-renderer', 'ytd-promoted-sparkles-web-renderer',
    'ytd-promoted-video-renderer', 'ytd-display-ad-renderer', 'ytd-companion-slot-renderer',
    'ytd-action-companion-ad-renderer', 'ytd-banner-promo-renderer', 'ytd-statement-banner-renderer',
    'ytd-video-masthead-ad-v3-renderer', '#masthead-ad', '#player-ads', '.ytp-ad-overlay-container',
    '.ytp-ad-image-overlay', 'ytd-merch-shelf-renderer', 'ytd-rich-item-renderer:has(ytd-ad-slot-renderer)',
    'ytd-rich-section-renderer:has(ytd-statement-banner-renderer)', 'ad-slot-renderer',
    'ytm-promoted-sparkles-web-renderer', 'ytm-companion-ad-renderer', 'ytd-enforcement-message-view-model',
    'tp-yt-paper-dialog:has(ytd-enforcement-message-view-model)',
  ].join(',') + '{display:none!important}';
  const addCss = () => {
    if (document.getElementById('__goatr_yt')) return;
    const s = document.createElement('style');
    s.id = '__goatr_yt';
    s.textContent = CSS;
    (document.head || document.documentElement).appendChild(s);
  };
  if (document.documentElement) addCss();
  document.addEventListener('DOMContentLoaded', addCss);

  // An ad that still plays: mute it, jump to its end, press Skip.
  let mutedByUs = false;
  setInterval(() => {
    const player = document.querySelector('.html5-video-player');
    if (!player) return;
    const v = player.querySelector('video');
    if (player.classList.contains('ad-showing') || player.classList.contains('ad-interrupting')) {
      if (v) {
        if (!v.muted) { v.muted = true; mutedByUs = true; }
        if (isFinite(v.duration) && v.duration > 0) v.currentTime = v.duration;
      }
      const skip = document.querySelector('.ytp-ad-skip-button, .ytp-ad-skip-button-modern, .ytp-skip-ad-button, button[class*="skip"]');
      if (skip) skip.click();
    } else if (mutedByUs && v) {
      v.muted = false;
      mutedByUs = false;
    }
    // The anti-ad-blocker notice pauses the video: carry on.
    if (document.querySelector('ytd-enforcement-message-view-model') && v && v.paused && !player.classList.contains('ad-showing')) {
      v.play().catch(() => {});
    }
  }, 250);
})();
