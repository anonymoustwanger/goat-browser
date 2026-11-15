'use strict';
// Loaded in <head> of internal pages: applies theme + motion before first paint.
(function () {
  const a = window.goatAppearance;
  const r = document.documentElement;
  r.dataset.theme = (a && a.theme) || (matchMedia('(prefers-color-scheme: light)').matches ? 'daylight' : 'midnight');
  r.dataset.motion = (a && a.motion) || 'system';
  r.dataset.gradients = a && a.gradients === false ? 'off' : 'on';
  if (a && a.vars) for (const k of Object.keys(a.vars)) r.style.setProperty(k, a.vars[k]);
})();
