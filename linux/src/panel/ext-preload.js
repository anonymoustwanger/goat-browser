'use strict';
const { contextBridge, ipcRenderer } = require('electron');

const listeners = { state: [], open: [] };
ipcRenderer.on('ext-state', (_e, s) => listeners.state.forEach((fn) => fn(s)));
ipcRenderer.on('panel-open', () => listeners.open.forEach((fn) => fn()));
ipcRenderer.on('appearance', (_e, a) => {
  const r = document.documentElement;
  r.dataset.theme = a.theme;
  r.dataset.motion = a.motion;
  r.dataset.gradients = a.gradients ? 'on' : 'off';
  r.removeAttribute('style');
  if (a.vars) for (const [k, v] of Object.entries(a.vars)) r.style.setProperty(k, v);
});

contextBridge.exposeInMainWorld('goatExt', {
  onState: (fn) => listeners.state.push(fn),
  onOpen: (fn) => listeners.open.push(fn),
  size: (h) => ipcRenderer.send('extmenu:size', h),
  close: () => ipcRenderer.send('extmenu:close'),
  pin: (id, on) => ipcRenderer.send('extmenu:pin', String(id), !!on),
  open: (id) => ipcRenderer.send('extmenu:open', String(id)),
  options: (id) => ipcRenderer.send('extmenu:options', String(id)),
  manage: () => ipcRenderer.send('extmenu:manage'),
  store: () => ipcRenderer.send('extmenu:store'),
});
