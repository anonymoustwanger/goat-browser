'use strict';
const { contextBridge, ipcRenderer } = require('electron');
contextBridge.exposeInMainWorld('setup', {
  info: () => ipcRenderer.invoke('setup:info'),
  chooseDir: (cur) => ipcRenderer.invoke('setup:choose-dir', String(cur || '')),
  install: (opts) => ipcRenderer.invoke('setup:install', opts),
  uninstall: (opts) => ipcRenderer.invoke('setup:uninstall', opts),
  launch: () => ipcRenderer.send('setup:launch'),
  close: () => ipcRenderer.send('setup:close'),
  minimize: () => ipcRenderer.send('setup:minimize'),
  onProgress: (fn) => ipcRenderer.on('setup:progress', (_e, p) => fn(p)),
});
