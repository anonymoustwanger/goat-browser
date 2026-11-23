'use strict';
const { contextBridge, ipcRenderer } = require('electron');

let onItems = () => {};
ipcRenderer.on('dl-items', (_e, items) => onItems(items));
ipcRenderer.on('appearance', (_e, a) => {
  const r = document.documentElement;
  r.dataset.theme = a.theme;
  r.dataset.motion = a.motion;
  r.removeAttribute('style');
  if (a.vars) for (const [k, v] of Object.entries(a.vars)) r.style.setProperty(k, v);
});

contextBridge.exposeInMainWorld('goatDl', {
  onItems: (fn) => { onItems = fn; },
  run: (id, action) => ipcRenderer.send('dlbar:do', Number(id), String(action)),
});
