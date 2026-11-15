'use strict';
const q = new URLSearchParams(location.search);
const url = q.get('u') || '';
const code = q.get('code') || '';
const desc = q.get('d') || '';
const n = Number(code);

let host = url;
try { host = new URL(url).host; } catch { /* keep raw */ }
document.getElementById('host').textContent = url;

let title = "Can't reach this page";
let why = `${host} didn't respond.`;
if (code === 'crashed') {
  title = 'This tab crashed';
  why = 'The page stopped working. Reloading usually fixes it.';
} else if (n <= -200 && n > -300) {
  title = 'Connection not trusted';
  why = `${host} sent a security certificate that can't be verified. Someone may be intercepting the connection.`;
} else if (n === -105 || n === -137) {
  why = `The address ${host} could not be found. Check the spelling. Goat resolves names over encrypted DNS (Quad9 / Mullvad); networks that block these servers can also cause this.`;
} else if (n === -102 || n === -101 || n === -100 || n === -118) {
  why = `${host} refused or dropped the connection.`;
  if (q.get('http') === '1') why += ' The site may not support HTTPS. Goat only opens plain-HTTP sites when you say so.';
} else if (n === -106) {
  why = "You're offline. Check your internet connection.";
} else if (n === -20 || n === -21) {
  title = 'Blocked';
  why = 'This request was blocked by Goat (tracker or ad domain).';
}
document.title = title;
document.getElementById('title').textContent = title;
document.getElementById('why').textContent = why;
document.getElementById('code').textContent = code === 'crashed' ? desc : `${desc || 'ERROR'} (${code})`;

document.getElementById('retry').addEventListener('click', () => { if (url) location.href = url; });
const httpBtn = document.getElementById('http');
if (q.get('http') === '1') {
  httpBtn.hidden = false;
  httpBtn.addEventListener('click', () => {
    location.href = 'goat://allow-http/?u=' + encodeURIComponent(url);
  });
}
