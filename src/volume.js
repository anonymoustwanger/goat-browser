// Goatr: per-tab volume (0–600%), set from Goatr's speaker button.
// Media goes through a Web Audio gain, so the page's own volume slider still works on top.
// Media from another site without CORS can't go through Web Audio (it would play silent), so it
// only has its volume turned down (never boosted).
(() => {
  if (window.__goatrVolume) return;
  let level = 1;
  let ctx = null;
  const hooked = new WeakMap(); // element -> GainNode

  const sameOrigin = (el) => {
    const src = el.currentSrc || el.src || '';
    if (!src || src.startsWith('blob:') || src.startsWith('data:') || src.startsWith('mediasource:')) return true;
    try { return new URL(src, location.href).origin === location.origin || el.crossOrigin != null; } catch (e) { return false; }
  };

  const boost = (el) => {
    if (hooked.has(el)) return hooked.get(el);
    if (!sameOrigin(el)) return null;
    try {
      ctx = ctx || new (window.AudioContext || window.webkitAudioContext)();
      const src = ctx.createMediaElementSource(el);
      const gain = ctx.createGain();
      src.connect(gain).connect(ctx.destination);
      hooked.set(el, gain);
      return gain;
    } catch (e) {
      return null;
    }
  };

  const apply = (el) => {
    const gain = hooked.get(el) || (level !== 1 ? boost(el) : null);
    if (gain) {
      gain.gain.value = level;
      if (ctx && ctx.state === 'suspended') ctx.resume().catch(() => {});
    } else {
      try { el.volume = Math.min(1, level); } catch (e) {}
    }
  };

  const all = () => {
    const found = [...document.querySelectorAll('video,audio')];
    // Media inside open shadow roots (some players).
    document.querySelectorAll('*').forEach((n) => { if (n.shadowRoot) found.push(...n.shadowRoot.querySelectorAll('video,audio')); });
    return found;
  };

  window.__goatrVolume = (v) => {
    level = Math.max(0, Math.min(6, Number(v) || 0));
    all().forEach(apply);
  };

  // New media (and media that starts later) gets the tab's level too.
  document.addEventListener('play', (e) => {
    if (level !== 1 && e.target instanceof HTMLMediaElement) apply(e.target);
  }, true);
})();
