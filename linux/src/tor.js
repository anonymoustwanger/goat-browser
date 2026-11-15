'use strict';
// Built-in Tor. On first use Goat downloads the official Tor Expert Bundle from
// torproject.org, checks its SHA-256 against the sums file signed by the Tor
// Browser team (OpenPGP, key fingerprint pinned below), unpacks it into the
// profile, and runs tor.exe in the background as a local SOCKS5 proxy.
// No Tor Browser needed.

const { session } = require('electron');
const { spawn } = require('child_process');
const crypto = require('crypto');
const fs = require('fs');
const net = require('net');
const path = require('path');
const zlib = require('zlib');

const DIST = 'https://dist.torproject.org/torbrowser/';
// Tor Browser Developers (signing key) <torbrowser@torproject.org>
const SIGNING_KEY_FPR = 'ef6e286dda85ea2a4ba7de684e2c6e8793298290';
const KEY_URLS = [
  'https://openpgpkey.torproject.org/.well-known/openpgpkey/torproject.org/hu/kounek7zrdx745qydx6p59t9mqjpuhdf?l=torbrowser',
  `https://keys.openpgp.org/vks/v1/by-fingerprint/${SIGNING_KEY_FPR.toUpperCase()}`,
];

function platformTag() {
  if (process.platform === 'win32') return process.arch === 'ia32' ? 'windows-i686' : 'windows-x86_64';
  if (process.platform === 'darwin') return process.arch === 'arm64' ? 'macos-aarch64' : 'macos-x86_64';
  return process.arch === 'ia32' ? 'linux-i686' : 'linux-x86_64';
}
const EXE = process.platform === 'win32' ? 'tor.exe' : 'tor';

// Minimal tar reader (ustar + GNU long names); enough for the expert bundle.
function untar(buf, dest) {
  let off = 0;
  let longName = null;
  while (off + 512 <= buf.length) {
    const h = buf.subarray(off, off + 512);
    if (h.every((b) => b === 0)) break;
    const str = (a, b) => h.subarray(a, b).toString('utf8').replace(/\0.*$/s, '');
    let name = str(0, 100);
    const prefix = str(345, 500);
    if (prefix && h.subarray(257, 262).toString() === 'ustar') name = prefix + '/' + name;
    const size = parseInt(str(124, 136).trim() || '0', 8);
    const type = String.fromCharCode(h[156] || 48);
    const mode = parseInt(str(100, 108).trim() || '644', 8);
    const body = buf.subarray(off + 512, off + 512 + size);
    off += 512 + Math.ceil(size / 512) * 512;
    if (type === 'L') { longName = body.toString('utf8').replace(/\0.*$/s, ''); continue; }
    if (longName) { name = longName; longName = null; }
    if (type === 'x' || type === 'g') continue; // pax headers
    const target = path.resolve(dest, name);
    if (!target.startsWith(path.resolve(dest) + path.sep)) continue; // no path escapes
    if (type === '5') fs.mkdirSync(target, { recursive: true });
    else if (type === '0' || type === '\0' || type === '7') {
      fs.mkdirSync(path.dirname(target), { recursive: true });
      fs.writeFileSync(target, body, { mode: mode & 0o755 });
    }
  }
}

function freePort(preferred) {
  const tryPort = (p) => new Promise((resolve) => {
    const srv = net.createServer();
    srv.once('error', () => resolve(null));
    srv.listen(p, '127.0.0.1', () => { const got = srv.address().port; srv.close(() => resolve(got)); });
  });
  return tryPort(preferred).then((p) => p || tryPort(0));
}

class Tor {
  constructor({ dir, stateDir, onChange }) {
    this.dir = dir; // %APPDATA%\Goat\tor (tor.exe + geoip)
    this.stateDir = stateDir || path.join(dir, 'state'); // Tor's own data (one per running Tor)
    this.onChange = onChange || (() => {});
    this.state = 'off'; // off | downloading | starting | on | error
    this.progress = 0;
    this.error = '';
    this.version = '';
    this.port = 0;
    this.proc = null;
    this.controlPort = 0;
    this.wanted = false;
    this._dl = null;
  }

  status() {
    return { state: this.state, progress: this.progress, error: this.error, version: this.version, port: this.port };
  }

  set(state, extra = {}) {
    Object.assign(this, { state }, extra);
    this.onChange(this.status());
  }

  exePath() { return path.join(this.dir, 'tor', EXE); }
  installed() { return fs.existsSync(this.exePath()); }

  // Network session for the download: never goes through the (not yet running) Tor proxy.
  dlSession() {
    if (!this._dl) {
      this._dl = session.fromPartition('goat-tor-download');
      this._dl.setProxy({ mode: 'system' }).catch(() => {});
    }
    return this._dl;
  }

  async fetchBuf(url, onProgress) {
    const res = await this.dlSession().fetch(url, { cache: 'no-store' });
    if (!res.ok) throw new Error(`${new URL(url).hostname} answered ${res.status}`);
    const total = Number(res.headers.get('content-length')) || 0;
    if (!onProgress || !res.body) return Buffer.from(await res.arrayBuffer());
    const reader = res.body.getReader();
    const parts = [];
    let got = 0;
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      parts.push(Buffer.from(value));
      got += value.length;
      if (total) onProgress(got / total);
    }
    return Buffer.concat(parts);
  }

  async signingKey(openpgp) {
    let lastErr = null;
    for (const url of KEY_URLS) {
      try {
        const buf = await this.fetchBuf(url);
        const text = buf.toString('utf8');
        const keys = text.includes('BEGIN PGP')
          ? await openpgp.readKeys({ armoredKeys: text })
          : await openpgp.readKeys({ binaryKeys: new Uint8Array(buf) });
        const key = keys.find((k) => k.getFingerprint().toLowerCase() === SIGNING_KEY_FPR);
        if (key) return key;
        lastErr = new Error('signing key fingerprint did not match');
      } catch (err) { lastErr = err; }
    }
    throw new Error(`couldn't get the Tor signing key (${lastErr && lastErr.message})`);
  }

  async install() {
    const openpgp = require('openpgp');
    this.set('downloading', { progress: 0, error: '' });
    const listing = (await this.fetchBuf(DIST)).toString('utf8');
    const versions = [...new Set([...listing.matchAll(/href="(\d+\.\d+(?:\.\d+)?)\/"/g)].map((m) => m[1]))]
      .sort((a, b) => { const x = a.split('.').map(Number); const y = b.split('.').map(Number); for (let i = 0; i < 3; i++) { if ((x[i] || 0) !== (y[i] || 0)) return (y[i] || 0) - (x[i] || 0); } return 0; });
    if (!versions.length) throw new Error('no Tor versions listed on torproject.org');
    const key = await this.signingKey(openpgp);
    const tag = platformTag();
    let lastErr = null;
    for (const v of versions.slice(0, 3)) {
      const file = `tor-expert-bundle-${tag}-${v}.tar.gz`;
      try {
        const sums = await this.fetchBuf(`${DIST}${v}/sha256sums-signed-build.txt`);
        const sig = await this.fetchBuf(`${DIST}${v}/sha256sums-signed-build.txt.asc`);
        const { signatures } = await openpgp.verify({
          message: await openpgp.createMessage({ binary: new Uint8Array(sums) }),
          signature: await openpgp.readSignature({ armoredSignature: sig.toString('utf8') }),
          verificationKeys: key,
        });
        await signatures[0].verified; // throws if the signature is bad
        const line = sums.toString('utf8').split('\n').find((l) => l.trim().endsWith(' ' + file));
        if (!line) throw new Error(`${file} not in the signed list`);
        const want = line.trim().split(/\s+/)[0].toLowerCase();
        const tgz = await this.fetchBuf(`${DIST}${v}/${file}`, (p) => { this.progress = Math.round(p * 100); this.onChange(this.status()); });
        const got = crypto.createHash('sha256').update(tgz).digest('hex');
        if (got !== want) throw new Error('download is corrupted (checksum mismatch)');
        const tmp = path.join(this.dir, 'unpack-' + Date.now());
        fs.mkdirSync(tmp, { recursive: true });
        untar(zlib.gunzipSync(tgz), tmp);
        for (const d of ['tor', 'data']) {
          fs.rmSync(path.join(this.dir, d), { recursive: true, force: true });
          if (fs.existsSync(path.join(tmp, d))) fs.renameSync(path.join(tmp, d), path.join(this.dir, d));
        }
        fs.rmSync(tmp, { recursive: true, force: true });
        if (!this.installed()) throw new Error('the bundle had no ' + EXE);
        fs.writeFileSync(path.join(this.dir, 'version.txt'), v);
        return v;
      } catch (err) { lastErr = err; }
    }
    throw lastErr || new Error('download failed');
  }

  async start() {
    this.wanted = true;
    if (this.proc || this.state === 'downloading' || this.state === 'starting') return;
    try {
      fs.mkdirSync(this.dir, { recursive: true });
      if (!this.installed()) await this.install();
      if (!this.wanted) { this.set('off'); return; }
      try { this.version = fs.readFileSync(path.join(this.dir, 'version.txt'), 'utf8').trim(); } catch { /* own tor.exe */ }
      this.port = await freePort(9155);
      this.set('starting', { progress: 0, error: '' });
      await this.launch();
    } catch (err) {
      this.set('error', { error: String((err && err.message) || err) });
    }
  }

  launch() {
    return new Promise((resolve) => {
      const data = path.join(this.dir, 'data');
      const portFile = path.join(this.stateDir, 'control-port');
      fs.mkdirSync(path.dirname(portFile), { recursive: true });
      try { fs.rmSync(portFile, { force: true }); } catch { /* ignore */ }
      const args = [
        '--ignore-missing-torrc', '-f', path.join(this.dir, 'torrc-none'),
        '--SocksPort', `127.0.0.1:${this.port}`,
        '--DataDirectory', this.stateDir,
        '--ControlPort', 'auto', '--ControlPortWriteToFile', portFile,
        '--CookieAuthentication', '1',
        '--Log', 'notice stdout',
        '--AvoidDiskWrites', '1',
        // Tor stops by itself if Goat goes away, even after a crash.
        '__OwningControllerProcess', String(process.pid),
      ];
      if (fs.existsSync(path.join(data, 'geoip'))) args.push('--GeoIPFile', path.join(data, 'geoip'));
      if (fs.existsSync(path.join(data, 'geoip6'))) args.push('--GeoIPv6File', path.join(data, 'geoip6'));
      const env = { ...process.env };
      if (process.platform === 'linux') env.LD_LIBRARY_PATH = path.join(this.dir, 'tor') + (env.LD_LIBRARY_PATH ? ':' + env.LD_LIBRARY_PATH : '');
      let proc;
      try {
        proc = spawn(this.exePath(), args, { cwd: path.join(this.dir, 'tor'), env, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
      } catch (err) { this.set('error', { error: String(err.message || err) }); resolve(); return; }
      this.proc = proc;
      let lastLine = '';
      const onData = (chunk) => {
        for (const line of String(chunk).split(/\r?\n/)) {
          if (!line.trim()) continue;
          lastLine = line.replace(/^.*\[(notice|warn|err)\]\s*/, '');
          const m = /Bootstrapped (\d+)%/.exec(line);
          if (m) {
            const pct = Number(m[1]);
            if (pct >= 100) { this.set('on', { progress: 100 }); resolve(); }
            else if (this.state === 'starting') { this.progress = pct; this.onChange(this.status()); }
          }
        }
      };
      proc.stdout.on('data', onData);
      proc.stderr.on('data', onData);
      proc.on('error', (err) => { this.proc = null; this.set('error', { error: String(err.message || err) }); resolve(); });
      proc.on('exit', (code) => {
        this.proc = null;
        if (this.wanted) this.set('error', { error: `Tor stopped (code ${code}). ${lastLine}`.trim() });
        else this.set('off', { progress: 0 });
        resolve();
      });
    });
  }

  stop() {
    this.wanted = false;
    const p = this.proc;
    this.proc = null;
    if (p) { try { p.kill(); } catch { /* gone */ } }
    if (this.state !== 'downloading') this.set('off', { progress: 0, error: '' });
  }

  // New Tor circuits: sites see a different exit IP for new connections.
  async newCircuit() {
    if (this.state !== 'on') return false;
    const portFile = path.join(this.stateDir, 'control-port');
    const cookieFile = path.join(this.stateDir, 'control_auth_cookie');
    const m = /PORT=([\d.]+):(\d+)/.exec(fs.readFileSync(portFile, 'utf8'));
    if (!m) return false;
    const cookie = fs.readFileSync(cookieFile).toString('hex');
    return new Promise((resolve) => {
      const sock = net.connect(Number(m[2]), m[1]);
      let out = '';
      sock.setTimeout(5000, () => { sock.destroy(); resolve(false); });
      sock.on('error', () => resolve(false));
      sock.on('data', (d) => {
        out += d;
        if ((out.match(/^250 /gm) || []).length >= 2 || /^5\d\d /m.test(out)) { sock.end('QUIT\r\n'); resolve(!/^5\d\d /m.test(out)); }
      });
      sock.write(`AUTHENTICATE ${cookie}\r\nSIGNAL NEWNYM\r\n`);
    });
  }

  kill() { this.wanted = false; if (this.proc) { try { this.proc.kill(); } catch { /* gone */ } } }
}

module.exports = { Tor, untar };
