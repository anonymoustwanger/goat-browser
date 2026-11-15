'use strict';
const { contextBridge, ipcRenderer } = require('electron');
const OUT = new Set(['navigate', 'new-tab', 'close-tab', 'activate', 'back', 'forward',
  'reload', 'stop', 'settings', 'tor-settings', 'profile-menu', 'update-open', 'tor-toggle', 'tor-menu', 'translate', 'translate-menu', 'shield', 'focus-page', 'find', 'find-stop',
  'tab-menu', 'group-menu', 'group-toggle', 'group-rename', 'move-tab',
  'merge-tabs', 'move-group', 'add-to-group',
  'bm-open', 'bm-star', 'bm-menu', 'bm-bar-menu', 'bm-folder', 'bm-overflow', 'bm-move', 'bm-rename', 'bm-add-tab', 'preconnect', 'new-tab-menu', 'app-menu']);
const IN = new Set(['state', 'focus-address', 'find-open', 'find-close', 'find-result', 'toast', 'rename-group', 'appearance', 'bm-rename-start']);

contextBridge.exposeInMainWorld('goat', {
  platform: process.platform,
  send: (channel, ...args) => { if (OUT.has(channel)) ipcRenderer.send(channel, ...args); },
  on: (channel, fn) => { if (IN.has(channel)) ipcRenderer.on(channel, (_e, ...args) => fn(...args)); },
});
