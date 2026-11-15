'use strict';
// Keep the original page's title in the tab while it sleeps.
try { document.title = new URLSearchParams(location.search).get('t') || 'Sleeping tab'; } catch (e) { /* ignore */ }
