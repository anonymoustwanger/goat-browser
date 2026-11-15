'use strict';
const { contextBridge, ipcRenderer } = require('electron');

const listeners = { state: [], open: [] };
ipcRenderer.on('menu-state', (_e, s) => listeners.state.forEach((fn) => fn(s)));
ipcRenderer.on('panel-open', () => listeners.open.forEach((fn) => fn()));
ipcRenderer.on('appearance', (_e, a) => {
  const r = document.documentElement;
  r.dataset.theme = a.theme;
  r.dataset.motion = a.motion;
  r.dataset.gradients = a.gradients ? 'on' : 'off';
  r.removeAttribute('style');
  if (a.vars) for (const [k, v] of Object.entries(a.vars)) r.style.setProperty(k, v);
});

contextBridge.exposeInMainWorld('goatMenu', {
  onState: (fn) => listeners.state.push(fn),
  onOpen: (fn) => listeners.open.push(fn),
  run: (action) => ipcRenderer.send('appmenu:do', String(action)),
  size: (h) => ipcRenderer.send('appmenu:size', h),
  close: () => ipcRenderer.send('appmenu:close'),
});
