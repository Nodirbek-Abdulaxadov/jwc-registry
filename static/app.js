// jwc-registry SPA — vanilla JS + hash routing. ~no deps.

const TOKEN_KEY = 'jwc-registry-token';
const $  = (s, root = document) => root.querySelector(s);
const $$ = (s, root = document) => Array.from(root.querySelectorAll(s));
const esc = (s) => String(s ?? '').replace(/[&<>"']/g, (c) =>
  ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

function getToken() { return localStorage.getItem(TOKEN_KEY); }
function setToken(t) { localStorage.setItem(TOKEN_KEY, t); }
function clearToken() { localStorage.removeItem(TOKEN_KEY); }

async function api(path, opts = {}) {
  const headers = { ...(opts.headers || {}) };
  const t = getToken();
  if (t) headers['Authorization'] = `Bearer ${t}`;
  if (opts.body && typeof opts.body === 'object' && !(opts.body instanceof FormData)) {
    headers['Content-Type'] = 'application/json';
    opts.body = JSON.stringify(opts.body);
  }
  const r = await fetch(path, { ...opts, headers });
  if (r.status === 401) {
    clearToken();
    refreshMe();
    throw new Error('401 not signed in (token cleared)');
  }
  if (!r.ok) throw new Error(`${r.status} ${await r.text()}`);
  if (r.status === 204) return null;
  const ct = r.headers.get('content-type') || '';
  return ct.includes('application/json') ? r.json() : r.text();
}

async function refreshMe() {
  if (!getToken()) {
    $('#who').textContent = '';
    $('#loginBtn').hidden = false;
    $('#logoutBtn').hidden = true;
    return null;
  }
  try {
    const me = await api('/api/v1/me');
    $('#who').textContent = me.email;
    $('#loginBtn').hidden = true;
    $('#logoutBtn').hidden = false;
    return me;
  } catch (_) {
    return null;
  }
}

function copyButton(text) {
  const btn = document.createElement('button');
  btn.className = 'copy';
  btn.textContent = 'Copy';
  btn.onclick = async () => {
    try { await navigator.clipboard.writeText(text); }
    catch (_) { /* ignore */ }
    btn.textContent = 'Copied';
    btn.classList.add('copied');
    setTimeout(() => { btn.textContent = 'Copy'; btn.classList.remove('copied'); }, 1200);
  };
  return btn;
}

function snippet(text) {
  const pre = document.createElement('pre');
  pre.className = 'snippet';
  const code = document.createElement('code');
  code.textContent = text;
  pre.appendChild(code);
  pre.appendChild(copyButton(text));
  return pre;
}

// =====================  VIEWS  =====================

async function viewPackages() {
  const root = $('#view');
  root.innerHTML = `<h1>Packages</h1><div id="list" class="panel"><div class="muted">Loading…</div></div>`;
  try {
    const pkgs = await fetch('/api/v1/pkg').then((r) => r.json());
    const list = $('#list');
    if (!pkgs.length) {
      list.innerHTML = '<div class="empty">No packages published yet. <a href="#/publish">Publish one →</a></div>';
      return;
    }
    list.innerHTML = `
      <table>
        <thead><tr><th>name</th><th>latest</th><th>owner</th><th></th></tr></thead>
        <tbody>${pkgs.map((p) => `
          <tr>
            <td><a href="#/p/${encodeURIComponent(p.name)}">${esc(p.name)}</a></td>
            <td class="mono">${esc(p.latest_version || '—')}</td>
            <td class="muted">${esc(p.owner_email)}</td>
            <td><a href="#/p/${encodeURIComponent(p.name)}">details →</a></td>
          </tr>`).join('')}
        </tbody>
      </table>`;
  } catch (e) {
    root.innerHTML += `<div class="alert err">${esc(e.message)}</div>`;
  }
}

async function viewPackage(name) {
  const root = $('#view');
  root.innerHTML = `<a href="#/" class="muted">← all packages</a>
                    <h1 class="mono">${esc(name)}</h1>
                    <div id="body"><div class="muted">Loading…</div></div>`;
  try {
    const pkg = await fetch(`/api/v1/pkg/${encodeURIComponent(name)}`).then((r) => {
      if (!r.ok) throw new Error(`${r.status}`);
      return r.json();
    });
    const latest = pkg.versions[0]?.version || '';
    const body = $('#body');
    body.innerHTML = `
      <div class="muted">Owner: <span class="mono">${esc(pkg.owner_email)}</span> ·
        ${pkg.versions.length} version(s)</div>

      <h2>Install</h2>
      <div id="quick"></div>

      <h2>Versions</h2>
      <div class="panel">
        <table>
          <thead><tr><th>version</th><th>sha256</th><th>size</th><th>uploaded</th><th></th></tr></thead>
          <tbody>${pkg.versions.map((v) => `
            <tr>
              <td class="mono">${esc(v.version)}</td>
              <td class="mono muted" title="${esc(v.sha256)}">${esc(v.sha256.slice(0, 12))}…</td>
              <td class="muted">${(v.size_bytes / 1024).toFixed(1)} KB</td>
              <td class="muted">${new Date(v.uploaded_at).toLocaleString()}</td>
              <td><a href="/api/v1/pkg/${encodeURIComponent(name)}/${encodeURIComponent(v.version)}/download">download</a></td>
            </tr>`).join('')}
          </tbody>
        </table>
      </div>`;

    const quick = $('#quick');
    quick.appendChild(makeQuickActions(name, latest));
  } catch (e) {
    $('#body').innerHTML = `<div class="alert err">${esc(e.message)}</div>`;
  }
}

function makeQuickActions(name, version) {
  const box = document.createElement('div');
  box.className = 'panel';
  box.innerHTML = `
    <div class="tabs" id="qa-tabs">
      <button data-tab="cli" class="active">jwc CLI</button>
      <button data-tab="manifest">jwcproj.json</button>
      <button data-tab="curl">curl</button>
    </div>
    <div id="qa-body"></div>`;
  const tabs = ['cli', 'manifest', 'curl'];
  const render = (tab) => {
    const body = box.querySelector('#qa-body');
    body.innerHTML = '';
    if (tab === 'cli') {
      body.appendChild(snippet(`jwc add ${name} --version "^${version}"`));
      body.appendChild(snippet('jwc install'));
    } else if (tab === 'manifest') {
      body.appendChild(snippet(
`{
  "dependencies": {
    "${name}": "^${version}"
  }
}`));
    } else {
      body.appendChild(snippet(
`curl -L https://registry-jwc.1kb.uz/api/v1/pkg/${name}/${version}/download \\
  -o ${name}-${version}.tar.gz`));
    }
    $$('button', box.querySelector('#qa-tabs')).forEach((b) =>
      b.classList.toggle('active', b.dataset.tab === tab));
  };
  $$('button', box.querySelector('#qa-tabs')).forEach((b) =>
    b.onclick = () => render(b.dataset.tab));
  render('cli');
  return box;
}

async function viewKeys() {
  const root = $('#view');
  if (!getToken()) {
    root.innerHTML = `<h1>API Keys</h1>
      <div class="alert warn">Sign in with Google to manage API keys.</div>`;
    return;
  }
  root.innerHTML = `
    <h1>API Keys</h1>
    <p class="muted">Long-lived tokens for <code>jwc publish</code> / CI pipelines.
      Each key is shown <strong>once</strong> on creation — copy it immediately.</p>

    <div class="panel">
      <div class="row">
        <input id="newKeyName" placeholder="key name (e.g. ci-publish)" maxlength="64">
        <button id="newKeyBtn" class="primary">Create key</button>
      </div>
    </div>

    <h2>Active keys</h2>
    <div id="keys"></div>`;

  $('#newKeyBtn').onclick = async () => {
    const name = $('#newKeyName').value.trim();
    if (!name) { return; }
    try {
      const r = await api('/api/v1/keys', { method: 'POST', body: { name } });
      $('#newKeyName').value = '';
      showPlaintextDialog(r);
      loadKeys();
    } catch (e) {
      alert(e.message);
    }
  };

  loadKeys();
}

async function loadKeys() {
  const box = $('#keys');
  box.innerHTML = '<div class="muted">Loading…</div>';
  try {
    const keys = await api('/api/v1/keys');
    if (!keys.length) {
      box.innerHTML = '<div class="empty">No active keys.</div>';
      return;
    }
    box.innerHTML = `
      <div class="panel">
        <table>
          <thead><tr><th>name</th><th>prefix</th><th>last used</th><th>created</th><th></th></tr></thead>
          <tbody>${keys.map((k) => `
            <tr data-id="${k.id}">
              <td>${esc(k.name)}</td>
              <td class="mono">${esc(k.prefix)}…</td>
              <td class="muted">${k.last_used_at ? new Date(k.last_used_at).toLocaleString() : 'never'}</td>
              <td class="muted">${new Date(k.created_at).toLocaleDateString()}</td>
              <td><button class="small danger" data-revoke="${k.id}">Revoke</button></td>
            </tr>`).join('')}
          </tbody>
        </table>
      </div>`;
    $$('button[data-revoke]', box).forEach((b) => {
      b.onclick = async () => {
        if (!confirm('Revoke this key permanently?')) return;
        await api(`/api/v1/keys/${b.dataset.revoke}`, { method: 'DELETE' });
        loadKeys();
      };
    });
  } catch (e) {
    box.innerHTML = `<div class="alert err">${esc(e.message)}</div>`;
  }
}

function showPlaintextDialog(r) {
  const dlg = document.createElement('dialog');
  dlg.innerHTML = `
    <h2 style="margin:0 0 8px">Save your API key — shown once</h2>
    <p class="muted">After closing this dialog the plaintext is gone forever.
      The registry only stores its SHA-256 hash.</p>`;
  dlg.appendChild(snippet(r.plaintext));
  const row = document.createElement('div');
  row.className = 'row';
  row.innerHTML = `<span class="grow muted">Name: <code>${esc(r.name)}</code></span>`;
  const close = document.createElement('button');
  close.className = 'primary';
  close.textContent = "I saved it — close";
  close.onclick = () => dlg.close();
  row.appendChild(close);
  dlg.appendChild(row);
  document.body.appendChild(dlg);
  dlg.showModal();
  dlg.addEventListener('close', () => dlg.remove());
}

async function viewPublish() {
  const root = $('#view');
  if (!getToken()) {
    root.innerHTML = `<h1>Publish</h1>
      <div class="alert warn">Sign in with Google to upload a package.</div>`;
    return;
  }
  root.innerHTML = `
    <h1>Publish</h1>
    <p class="muted">Upload a <code>tar.gz</code> of your package directory.
      Or use <a href="#/keys">an API key</a> with <code>jwc publish</code>.</p>
    <div class="panel">
      <div class="col">
        <input id="pname" placeholder="package name (e.g. logger)">
        <input id="pver" placeholder="version (e.g. 0.1.0)">
        <input id="pfile" type="file" accept=".tar.gz,.tgz">
        <div class="row">
          <button id="puploadBtn" class="primary">Upload</button>
          <span id="presult" class="grow"></span>
        </div>
      </div>
    </div>

    <h2>Pack from your terminal</h2>
    <p class="muted">From the directory <em>containing</em> your package folder:</p>`;
  root.appendChild(snippet('tar -czf my-pkg-0.1.0.tar.gz -C my-pkg .'));

  $('#puploadBtn').onclick = async () => {
    const name = $('#pname').value.trim();
    const version = $('#pver').value.trim();
    const file = $('#pfile').files[0];
    const out = $('#presult');
    if (!name || !version || !file) {
      out.innerHTML = '<span class="alert err">all three required</span>';
      return;
    }
    const fd = new FormData();
    fd.append('file', file);
    try {
      const r = await api(`/api/v1/pkg/${encodeURIComponent(name)}/${encodeURIComponent(version)}`,
        { method: 'POST', body: fd });
      out.innerHTML = `<span class="alert ok">Published ${esc(r.name)}@${esc(r.version)} —
        <a href="#/p/${encodeURIComponent(r.name)}">view</a></span>`;
    } catch (e) {
      out.innerHTML = `<span class="alert err">${esc(e.message)}</span>`;
    }
  };
}

// =====================  ROUTER  =====================

function route() {
  const hash = window.location.hash.slice(1) || '/';
  const [_, head, ...rest] = hash.split('/');
  $$('header nav a').forEach((a) => a.classList.toggle('active',
    a.dataset.route === '/' + (head || '')));
  if (head === 'p' && rest[0]) return viewPackage(decodeURIComponent(rest[0]));
  if (head === 'keys')    return viewKeys();
  if (head === 'publish') return viewPublish();
  return viewPackages();
}

// =====================  BOOT  =====================

(function init() {
  $('#loginBtn').onclick = () => { window.location.href = '/api/v1/auth/google/login'; };
  $('#logoutBtn').onclick = () => { clearToken(); refreshMe(); route(); };

  // OAuth callback hands us back `?token=...` — stash + clean.
  const url = new URL(window.location.href);
  const tok = url.searchParams.get('token');
  if (tok) {
    setToken(tok);
    history.replaceState({}, '', '/' + (window.location.hash || ''));
  }

  window.addEventListener('hashchange', route);
  refreshMe().then(route);
})();
