'use strict';
// Runs in every frame, but only exposes an API to Goat's own internal pages.
// The main process re-checks the sender's URL on every call.
const { contextBridge, ipcRenderer } = require('electron');

const isTop = window === window.top;
const host = location.protocol === 'goat:' && isTop ? location.host : null;

if (host) {
  // Theme + motion for internal pages, read synchronously so there is no flash.
  let look = null;
  try { look = ipcRenderer.sendSync('goat:appearance-get'); } catch { /* ignore */ }
  contextBridge.exposeInMainWorld('goatAppearance', look);
  ipcRenderer.on('goat:appearance', (_e, a) => {
    const r = document.documentElement;
    r.dataset.theme = a.theme;
    r.dataset.motion = a.motion;
    r.dataset.gradients = a.gradients ? 'on' : 'off';
    r.removeAttribute('style');
    if (a.vars) for (const [k, v] of Object.entries(a.vars)) r.style.setProperty(k, v);
  });
}

if (host === 'history') {
  const call = (ch) => (...a) => ipcRenderer.invoke('goat:' + ch, ...a);
  contextBridge.exposeInMainWorld('goat', {
    query: call('history-query'),
    remove: call('history-delete'),
    clear: call('history-clear'),
    open: call('history-open'),
    settings: call('history-settings'),
  });
} else if (host === 'newtab') {
  contextBridge.exposeInMainWorld('goat', {
    search: (q) => ipcRenderer.invoke('goat:newtab-search', String(q)),
    bookmarks: () => ipcRenderer.invoke('goat:newtab-bookmarks'),
    open: (id, how) => ipcRenderer.invoke('goat:newtab-open', String(id), how),
    add: (url, title) => ipcRenderer.invoke('goat:newtab-add', String(url), String(title || '')),
    remove: (id) => ipcRenderer.invoke('goat:newtab-remove', String(id)),
    onBookmarks: (fn) => ipcRenderer.on('goat:bookmarks', (_e, items) => fn(items)),
  });
} else if (host === 'settings') {
  const call = (ch) => (...a) => ipcRenderer.invoke('goat:' + ch, ...a);
  contextBridge.exposeInMainWorld('goat', {
    getSettings: call('settings-get'),
    setSettings: call('settings-set'),
    blockerStatus: call('blocker-status'),
    refreshBlocklists: call('blocker-refresh'),
    cookies: {
      list: call('cookies-list'),
      remove: call('cookies-remove'),
      set: call('cookies-set'),
      clear: call('cookies-clear'),
    },
    clearBrowsingData: call('clear-data'),
    uninstall: call('uninstall'),
    profiles: {
      list: call('profiles'), create: call('profile-create'), update: call('profile-update'),
      remove: call('profile-delete'), open: call('profile-open'), shortcut: call('profile-shortcut'),
    },
    updates: {
      status: call('update-status'), check: call('update-check'), install: call('update-install'),
      onStatus: (fn) => ipcRenderer.on('goat:update', (_e, u) => fn(u)),
    },
    tasks: { list: call('tasks'), sleep: call('task-sleep'), close: call('task-close') },
    dns: { status: call('dns-status'), openCheck: call('dns-check') },
    tor: {
      status: call('tor-status'), newCircuit: call('tor-newcircuit'), check: call('tor-check'),
      onStatus: (fn) => ipcRenderer.on('goat:tor', (_e, st) => fn(st)),
    },
    extensions: {
      list: call('ext-list'),
      remove: call('ext-remove'),
      loadUnpacked: call('ext-load-unpacked'),
      openWebStore: call('ext-webstore'),
      openOptions: call('ext-options'),
      pin: call('ext-pin'),
      onChanged: (fn) => ipcRenderer.on('goat:extensions-changed', () => fn()),
    },
  });
}
