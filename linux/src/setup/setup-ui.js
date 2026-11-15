'use strict';
const $ = (id) => document.getElementById(id);
const api = window.setup;
let info = null;
let dir = '';
let lastAction = null;

function show(id) { for (const s of document.querySelectorAll('.step')) s.hidden = s.id !== id; }

api.onProgress((p) => {
  $('p-step').textContent = p.step;
  $('p-bar').style.width = p.pct + '%';
  $('p-pct').textContent = p.pct + '%';
});

async function doInstall() {
  lastAction = doInstall;
  $('p-title').textContent = info.existing ? 'Updating…' : 'Installing…';
  show('s-work');
  const r = await api.install({ dir, desktop: $('o-desktop').checked, makeDefault: $('o-default').checked });
  if (!r.ok) { $('e-msg').textContent = r.error; show('s-error'); return; }
  $('d-title').textContent = info.existing ? 'Goatr is updated' : 'Goatr is ready';
  $('d-sub').textContent = info.existing ? `You're on version ${info.version}. Your tabs and settings are kept.` : 'Find it in the Start menu any time.';
  $('d-open').hidden = false;
  show('s-done');
}

async function doUninstall() {
  lastAction = doUninstall;
  $('p-title').textContent = 'Uninstalling…';
  show('s-work');
  const r = await api.uninstall({ removeProfile: $('o-profile').checked });
  if (!r.ok) { $('e-msg').textContent = r.error; show('s-error'); return; }
  $('d-title').textContent = 'Goatr was removed';
  $('d-sub').textContent = $('o-profile').checked ? 'Your profile was deleted too. Thanks for trying Goatr.' : 'Your profile was kept, in case you come back. Thanks for trying Goatr.';
  $('d-open').hidden = true;
  show('s-done');
}

$('min').onclick = () => api.minimize();
$('x').onclick = () => api.close();
$('w-cancel').onclick = () => api.close();
$('c-cancel').onclick = () => api.close();
$('d-close').onclick = () => api.close();
$('e-close').onclick = () => api.close();
$('d-open').onclick = () => api.launch();
$('w-go').onclick = doInstall;
$('c-go').onclick = doUninstall;
$('e-retry').onclick = () => (lastAction ? lastAction() : null);
$('o-change').onclick = async () => {
  const d = await api.chooseDir(dir);
  if (d) { dir = d; $('o-dir').textContent = d; $('o-dir').title = d; }
};

(async () => {
  info = await api.info();
  document.documentElement.dataset.theme = info.dark ? 'midnight' : 'daylight';
  if (info.mode === 'uninstall') {
    $('bartitle').textContent = 'Uninstall Goatr';
    $('c-sub').textContent = `Version ${info.version} will be removed from this computer.`;
    $('c-profile').textContent = info.profileDir;
    show('s-confirm');
  } else {
    dir = info.dir;
    $('o-dir').textContent = dir; $('o-dir').title = dir;
    if (info.existing) {
      $('w-title').textContent = 'Update Goatr';
      $('w-sub').textContent = info.existingVersion && info.existingVersion !== info.version
        ? `Version ${info.existingVersion} → ${info.version}. Your tabs, cookies and settings are kept.`
        : `Reinstall version ${info.version}. Your tabs, cookies and settings are kept.`;
      $('w-go').textContent = 'Update';
      $('o-change').hidden = true;
    } else {
      $('w-sub').textContent = `Version ${info.version} · Private, fast, and yours. Takes about a minute.`;
    }
    if (info.platform !== 'win32') $('o-default').closest('.opt').hidden = true;
    show('s-welcome');
  }
})();
