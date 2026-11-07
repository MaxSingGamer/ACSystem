// AlphaCoin 服务中心面板前端（v4.0.0：登录 / 强制改密 / 安全中心 / 双域名）
// 纯原生 JS：无构建步骤、随 exe 内嵌分发。
'use strict';

const $ = (id) => document.getElementById(id);
const esc = (s) => String(s == null ? '' : s)
  .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
  .replace(/"/g, '&quot;').replace(/'/g, '&#39;');
const fmtBytes = (n) => {
  n = Number(n || 0);
  if (n < 1024) return n + ' B';
  if (n < 1024 * 1024) return (n / 1024).toFixed(1) + ' KB';
  if (n < 1024 * 1024 * 1024) return (n / 1024 / 1024).toFixed(2) + ' MB';
  return (n / 1024 / 1024 / 1024).toFixed(2) + ' GB';
};
function toast(msg, isErr) {
  const t = $('toast');
  t.textContent = msg;
  t.style.borderColor = isErr ? '#7a2530' : '';
  t.classList.remove('hidden');
  clearTimeout(toast._t);
  toast._t = setTimeout(() => t.classList.add('hidden'), isErr ? 6000 : 2800);
}

// ---------------- 会话 / 请求 ----------------
let TOKEN = localStorage.getItem('acs_panel_token') || '';
let STATUS = null;
let SERVICES = [];
let PENDING_TOTP = null;

async function api(path, opts = {}) {
  const headers = Object.assign({ 'Content-Type': 'application/json' }, opts.headers || {});
  if (TOKEN) headers['Authorization'] = 'Bearer ' + TOKEN;
  const r = await fetch(path, Object.assign({}, opts, { headers }));
  let data = null;
  const ct = r.headers.get('content-type') || '';
  if (ct.includes('application/json')) {
    try { data = await r.json(); } catch (e) { data = null; }
  }
  if (!r.ok) {
    const e = new Error((data && data.error) ? data.error : ('HTTP ' + r.status));
    e.status = r.status;
    e.data = data || {};
    throw e;
  }
  return data;
}

function setToken(t) {
  TOKEN = t || '';
  if (TOKEN) localStorage.setItem('acs_panel_token', TOKEN);
  else localStorage.removeItem('acs_panel_token');
}

// ---------------- 弹窗 ----------------
function openModal(title, bodyHtml, onOk, okText, cancelText) {
  $('modalTitle').textContent = title;
  $('modalBody').innerHTML = bodyHtml;
  $('modalError').textContent = '';
  $('modalWrap').classList.remove('hidden');
  $('modalOk').textContent = okText || '确定';
  $('modalOk').classList.toggle('hidden', !onOk);
  $('modalCancel').classList.toggle('hidden', !onOk && !cancelText);
  $('modalCancel').textContent = cancelText || '取消';
  $('modalOk').onclick = onOk || null;
  $('modalCancel').onclick = closeModal;
  $('modalClose').onclick = closeModal;
}
function closeModal() { $('modalWrap').classList.add('hidden'); $('modalBody').innerHTML = ''; }

// ---------------- 登录 / 强制改密 ----------------
function showAuth() {
  $('authView').classList.remove('hidden');
  $('appView').classList.add('hidden');
  $('loginPane').classList.remove('hidden');
  $('changePane').classList.add('hidden');
  // 未登录状态只返回 passkey_available（见 /api/auth/status 的最小化返回）
  const canPasskey = !!(STATUS && (STATUS.passkey_available || STATUS.passkey_count > 0));
  $('btnPasskey').classList.toggle('hidden', !canPasskey);
}
function showChange() {
  $('authView').classList.remove('hidden');
  $('appView').classList.add('hidden');
  $('loginPane').classList.add('hidden');
  $('changePane').classList.remove('hidden');
  $('cErr').textContent = '';
}
function showApp() {
  $('authView').classList.add('hidden');
  $('appView').classList.remove('hidden');
  loadServices().catch(e => toast(e.message, true));
}

async function boot() {
  try { STATUS = await api('/api/auth/status'); }
  catch (e) { STATUS = null; }
  if (!STATUS || !STATUS.logged_in) {
    setToken('');
    showAuth();
  } else if (STATUS.must_change) {
    showChange();
  } else {
    showApp();
  }
}

async function doLogin() {
  $('lErr').textContent = '';
  const username = $('lUser').value.trim();
  const password = $('lPass').value;
  const totp_code = $('lTotp').value.trim();
  if (!username || !password) { $('lErr').textContent = '请输入用户名与密码'; return; }
  try {
    const r = await api('/api/auth/login', { method: 'POST', body: JSON.stringify({ username, password, totp_code }) });
    setToken(r.token);
    STATUS = await api('/api/auth/status');
    $('lPass').value = ''; $('lTotp').value = '';
    if (r.must_change) showChange(); else showApp();
  } catch (e) {
    if (e.data && e.data.need_totp) {
      $('lTotpWrap').classList.remove('hidden');
      $('lTotp').focus();
    }
    $('lErr').textContent = e.message;
  }
}

async function doChange() {
  $('cErr').textContent = '';
  const new_username = $('cUser').value.trim();
  const new_password = $('cPass').value;
  const confirm_password = $('cPass2').value;
  if (!new_username) { $('cErr').textContent = '请填写新用户名'; return; }
  if (new_password.length < 8) { $('cErr').textContent = '新密码至少 8 位'; return; }
  if (new_password !== confirm_password) { $('cErr').textContent = '两次输入的新密码不一致'; return; }
  try {
    const r = await api('/api/auth/change-credentials', {
      method: 'POST', body: JSON.stringify({ new_username, new_password, confirm_password })
    });
    STATUS = await api('/api/auth/status');
    toast(r.message || '凭据已更新');
    $('cPass').value = ''; $('cPass2').value = '';
    showApp();
  } catch (e) { $('cErr').textContent = e.message; }
}

// Passkey 一键登录
const b64uEncode = (buf) => {
  const u = new Uint8Array(buf);
  let s = '';
  for (let i = 0; i < u.length; i++) s += String.fromCharCode(u[i]);
  return btoa(s).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
};
const b64uDecode = (s) => {
  s = String(s).replace(/-/g, '+').replace(/_/g, '/');
  while (s.length % 4) s += '=';
  const bin = atob(s);
  const u = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) u[i] = bin.charCodeAt(i);
  return u;
};
function passkeySupported() { return !!(window.PublicKeyCredential && window.isSecureContext); }

async function doPasskeyLogin() {
  $('lErr').textContent = '';
  if (!passkeySupported()) { $('lErr').textContent = '当前环境不支持 Passkey（需通过 HTTPS 面板域名访问）'; return; }
  try {
    const b = await api('/api/auth/passkey/login/begin', { method: 'POST' });
    const pk = {
      challenge: b64uDecode(b.challenge),
      rpId: b.rpId,
      allowCredentials: (b.allowCredentials || []).map(c => ({ type: 'public-key', id: b64uDecode(c.id) })),
      timeout: b.timeout,
      userVerification: b.userVerification || 'preferred',
    };
    const cred = await navigator.credentials.get({ publicKey: pk });
    const resp = cred.response;
    const body = {
      id: b64uEncode(cred.rawId),
      clientDataJSON: b64uEncode(resp.clientDataJSON),
      authenticatorData: b64uEncode(resp.authenticatorData),
      signature: b64uEncode(resp.signature),
    };
    const r = await api('/api/auth/passkey/login/finish', { method: 'POST', body: JSON.stringify(body) });
    setToken(r.token);
    STATUS = await api('/api/auth/status');
    if (r.must_change) showChange(); else showApp();
  } catch (e) {
    $('lErr').textContent = e.message || 'Passkey 登录失败';
  }
}

// ---------------- 再次验证密码 ----------------
function askPassword(msg) {
  return new Promise((resolve) => {
    openModal('验证登录密码', `
      <div class="notice">${esc(msg || '该操作涉及账户安全，请再次输入登录密码。')}</div>
      <div class="field"><label>登录密码</label><input id="vpPass" type="password" autocomplete="current-password"></div>
    `, async () => {
      const p = $('vpPass').value;
      if (!p) { $('modalError').textContent = '请输入密码'; return; }
      try {
        await api('/api/auth/verify', { method: 'POST', body: JSON.stringify({ password: p }) });
        if (STATUS) STATUS.verified = true;
        closeModal(); resolve(true);
      } catch (e) { $('modalError').textContent = e.message; }
    }, '验证');
    $('modalCancel').onclick = () => { closeModal(); resolve(false); };
    $('modalClose').onclick = () => { closeModal(); resolve(false); };
  });
}
async function requireVerified(msg) {
  if (STATUS && STATUS.verified) return true;
  return await askPassword(msg);
}

// ---------------- 服务列表 ----------------
async function loadServices() {
  const d = await api('/api/services');
  SERVICES = d.services || [];
  const p = d.panel || {};
  $('statCount').textContent = SERVICES.length;
  $('statRunning').textContent = SERVICES.filter(s => s.state === 'running').length;
  $('statPanelPort').textContent = p.port ?? '–';
  $('statRoot').textContent = p.root || '—';
  $('panelAddr').textContent = p.url || '';
  $('panelAddr').title = p.url || '';
  const body = $('svcBody');
  if (!SERVICES.length) {
    body.innerHTML = '<tr><td colspan="7" class="muted">还没有服务：点右上角「新建服务」，或「导入服务包」。</td></tr>';
    return;
  }
  body.innerHTML = SERVICES.map(s => `
    <tr>
      <td><b>${esc(s.name)}</b></td>
      <td class="mono">${esc(s.api_domain)}${s.dir_mismatch ? ' <span class="tag warn" title="文件夹名与服务接口域名不一致">命名不一致</span>' : ''}</td>
      <td class="mono">${esc(s.admin_domain)}</td>
      <td class="mono">${s.api_port}</td>
      <td class="mono">${s.admin_port}</td>
      <td><span class="tag ${esc(s.state)}">${stateText(s.state)}</span></td>
      <td><div class="ops">
        ${s.state === 'running'
          ? `<button class="btn btn-sm" data-act="stop" data-n="${esc(s.name)}">停止</button>`
          : `<button class="btn btn-sm btn-primary" data-act="start" data-n="${esc(s.name)}">启动</button>`}
        <button class="btn btn-sm" data-act="logs" data-n="${esc(s.name)}">日志</button>
        <button class="btn btn-sm" data-act="edit" data-n="${esc(s.name)}">改域名/端口</button>
        ${s.dir_mismatch ? `<button class="btn btn-sm btn-warn" data-act="resolve" data-n="${esc(s.name)}">统一命名</button>` : ''}
        <button class="btn btn-sm" data-act="export" data-n="${esc(s.name)}">导出</button>
        <button class="btn btn-sm" data-act="open" data-n="${esc(s.name)}">后台</button>
        <button class="btn btn-sm btn-danger" data-act="del" data-n="${esc(s.name)}">删除</button>
      </div></td>
    </tr>`).join('');
  body.querySelectorAll('button[data-act]').forEach(b => {
    b.onclick = () => handleAct(b.dataset.act, b.dataset.n, b);
  });
}
function stateText(s) { return s === 'running' ? '运行中' : (s === 'starting' ? '启动中' : '已停止'); }

async function handleAct(act, name, btn) {
  const svc = SERVICES.find(s => s.name === name);
  if (!svc) return;
  try {
    if (act === 'start') {
      btn.disabled = true; btn.textContent = '启动中…';
      const r = await api(`/api/services/${encodeURIComponent(name)}/start`, { method: 'POST' });
      toast(r.message || '已启动');
      setTimeout(loadServices, 1200);
    } else if (act === 'stop') {
      if (!confirm(`确定停止服务「${name}」？（不会删除数据）`)) return;
      const r = await api(`/api/services/${encodeURIComponent(name)}/stop`, { method: 'POST' });
      toast(r.message || '已停止'); loadServices();
    } else if (act === 'logs') {
      showLogs(name);
    } else if (act === 'edit') {
      showEdit(svc);
    } else if (act === 'resolve') {
      showResolveDir(svc);
    } else if (act === 'export') {
      showExport(svc);
    } else if (act === 'open') {
      window.open(svc.admin_url, '_blank');
    } else if (act === 'del') {
      if (!confirm(`确定删除服务「${name}」？\n\n这将停止服务并**永久删除**其数据目录（不可恢复）。\n建议先导出 AlphaTar 服务包备份。`)) return;
      const r = await api(`/api/services/${encodeURIComponent(name)}`, { method: 'DELETE' });
      toast(r.message || '已删除'); loadServices();
    }
  } catch (e) { toast(e.message, true); btn.disabled = false; btn.textContent = act === 'start' ? '启动' : btn.textContent; }
}

// ---------------- 新建服务 ----------------
function showCreate() {
  openModal('新建服务', `
    <div class="field"><label>服务名称（唯一，用于客户端历史区分）</label>
      <input id="nName" placeholder="如 AlphaEU 主中心"></div>
    <div class="grid2">
      <div class="field"><label>服务接口域名（API；唯一识别符，同时决定文件夹名）</label><input id="nApiDomain" placeholder="如 acsystem.maxshin.top"></div>
      <div class="field"><label>后台管理域名（网页；留空默认 admin.接口域名）</label><input id="nAdminDomain" placeholder="如 admin.maxshin.top"></div>
      <div class="field"><label>API 端口（无默认值，必填）</label><input id="nApi" type="number" placeholder="如 9700"></div>
      <div class="field"><label>管理端口（无默认值，必填）</label><input id="nAdmin" type="number" placeholder="如 9701"></div>
      <div class="field"><label>品牌名（可选）</label><input id="nBrand" placeholder="Alpha Coin"></div>
    </div>
    <div class="field"><label>根管理员账户（每行一个 <code>uid:密码</code>，至少一个；首启后不可增减）</label>
      <textarea id="nRoots" placeholder="Max_Shin:ChangeMe123!"></textarea></div>
    <div class="field"><label>系统账户（每行一个 <code>uid:密码</code>，可留空）</label>
      <textarea id="nSystems" placeholder="PreIssuedAccount:ChangeMe123!&#10;AESystem:ChangeMe123!"></textarea></div>
    <div class="field"><label>发行账户 uid（填了系统账户则必填，须为其中之一）</label>
      <input id="nPre" placeholder="PreIssuedAccount"></div>
    <div class="warn-line">两个域名都必须唯一；端口必须与其他服务及面板端口不同；新建后只允许修改域名与端口。</div>
  `, async () => {
    const req = {
      name: $('nName').value.trim(),
      api_domain: $('nApiDomain').value.trim(),
      admin_domain: $('nAdminDomain').value.trim(),
      api_port: Number($('nApi').value),
      admin_port: Number($('nAdmin').value),
      root_accounts: splitLines($('nRoots').value),
      system_accounts: splitLines($('nSystems').value),
      pre_issued: $('nPre').value.trim(),
      brand_name: $('nBrand').value.trim(),
      brand_currency: '',
    };
    if (!req.name || !req.api_domain || !req.api_port || !req.admin_port) {
      $('modalError').textContent = '名称、服务接口域名、API 端口、管理端口均为必填'; return;
    }
    try {
      await api('/api/services', { method: 'POST', body: JSON.stringify(req) });
      closeModal(); toast('服务已创建，可点「启动」运行'); loadServices();
    } catch (e) { $('modalError').textContent = e.message; }
  }, '创建');
}
function splitLines(t) {
  return String(t || '').split(/\r?\n/).map(s => s.trim()).filter(Boolean);
}

// ---------------- 改域名/端口（自动停启） ----------------
function showEdit(svc) {
  openModal(`修改服务「${svc.name}」`, `
    <div class="notice">仅允许修改<b>域名</b>与<b>端口</b>。修改会同步：文件夹名（按服务接口域名）→ servers.json → 该服务 .env。
    ${svc.state === 'running' ? '<br>服务当前<b>正在运行</b>：保存时会<b>自动停止 → 改名 → 自动重启</b>（释放文件夹占用）。' : ''}</div>
    <div class="field"><label>服务接口域名（API，唯一识别符）</label><input id="eApiDomain" value="${esc(svc.api_domain)}"></div>
    <div class="field"><label>后台管理域名（网页入口）</label><input id="eAdminDomain" value="${esc(svc.admin_domain)}"></div>
    <div class="grid2">
      <div class="field"><label>API 端口</label><input id="eApi" type="number" value="${svc.api_port}"></div>
      <div class="field"><label>管理端口</label><input id="eAdmin" type="number" value="${svc.admin_port}"></div>
    </div>
  `, async () => {
    const req = {
      api_domain: $('eApiDomain').value.trim(),
      admin_domain: $('eAdminDomain').value.trim(),
      api_port: Number($('eApi').value),
      admin_port: Number($('eAdmin').value),
    };
    if (!req.api_domain) { $('modalError').textContent = '服务接口域名不能为空'; return; }
    try {
      const r = await api(`/api/services/${encodeURIComponent(svc.name)}`, { method: 'PUT', body: JSON.stringify(req) });
      closeModal(); toast(r.message || '已更新'); loadServices();
    } catch (e) { $('modalError').textContent = e.message; }
  }, '保存');
}

// ---------------- 文件夹名与接口域名不一致：选择 ----------------
function showResolveDir(svc) {
  openModal(`统一「${svc.name}」的文件夹名与服务接口域名`, `
    <div class="notice">当前两者不一致：<br>
      文件夹名：<code>${esc(svc.dir)}</code><br>
      服务接口域名：<code>${esc(svc.api_domain)}</code></div>
    <div class="field"><label><input type="radio" name="rd" value="domain" checked style="width:auto"> 使用服务接口域名（重命名文件夹为 <code>${esc(svc.api_domain)}</code>）</label></div>
    <div class="field"><label><input type="radio" name="rd" value="dir" style="width:auto"> 使用文件夹名（把服务接口域名改为 <code>${esc(svc.dir)}</code>）</label></div>
    <div class="warn-line">服务若在运行，会先自动停止再统一命名，随后自动重启。</div>
  `, async () => {
    const choice = document.querySelector('input[name="rd"]:checked').value;
    try {
      const r = await api(`/api/services/${encodeURIComponent(svc.name)}/resolve-dir`, {
        method: 'POST', body: JSON.stringify({ choice })
      });
      closeModal(); toast(r.message || '已统一命名'); loadServices();
    } catch (e) { $('modalError').textContent = e.message; }
  }, '执行');
}

// ---------------- 导出服务包 ----------------
function showExport(svc) {
  openModal(`导出 AlphaTar 服务包「${svc.name}」`, `
    <div class="notice">导出的是<b>整个服务数据目录</b>（含数据库 / .env / 日志），文件名：
      <code>${esc(svc.api_domain)}.gz.alphatar</code>。服务必须先停止，保证数据库快照一致。</div>
    <div class="field"><label>加密密码（可选；留空 = 普通 tar.gz）</label>
      <input id="xPass" type="password" placeholder="留空则不加密"></div>
    <div class="field"><label>再次输入密码</label><input id="xPass2" type="password" placeholder="与上面一致"></div>
    <div class="hint">密码不落盘；导入时需要输入同一密码。</div>
  `, async () => {
    const p1 = $('xPass').value, p2 = $('xPass2').value;
    if (p1 !== p2) { $('modalError').textContent = '两次输入的密码不一致'; return; }
    const q = p1 ? ('?password=' + encodeURIComponent(p1)) : '';
    closeModal();
    toast('正在导出，浏览器将开始下载…');
    window.location = `/api/services/${encodeURIComponent(svc.name)}/export${q}`;
  }, '开始导出');
}

// ---------------- 导入服务包 ----------------
function showImport() {
  openModal('导入 AlphaTar 服务包', `
    <div class="notice">包内的服务接口域名 / 后台管理域名 / 端口会自动读取；<b>服务名称不打包</b>，需在此设置（保证唯一）。
    若文件夹名与包内服务接口域名不一致，会要求选择以哪一个为准。</div>
    <div class="field"><label>服务包文件（.gz.alphatar）</label><input id="iFile" type="file" accept=".alphatar,application/octet-stream"></div>
    <div class="field"><label>服务名称（唯一）</label><input id="iName" placeholder="如 AlphaEU 备用中心"></div>
    <div class="field"><label>解密密码（若导出时设置了）</label><input id="iPass" type="password" placeholder="未加密则留空"></div>
  `, async () => {
    const f = $('iFile').files[0];
    const name = $('iName').value.trim();
    const pass = $('iPass').value;
    if (!f) { $('modalError').textContent = '请选择服务包文件'; return; }
    if (!name) { $('modalError').textContent = '请填写服务名称'; return; }
    try {
      const buf = await f.arrayBuffer();
      await submitImport(buf, name, pass, '');
      closeModal(); toast('导入完成'); loadServices();
    } catch (e) {
      if (e.data && e.data.needs_dir_choice) {
        showImportDirChoice(e.data, f, name, pass);
      } else { $('modalError').textContent = e.message; }
    }
  }, '导入');
}

async function submitImport(buf, name, pass, dirChoice) {
  const q = new URLSearchParams({ name, password: pass });
  if (dirChoice) q.set('dir_choice', dirChoice);
  return await api(`/api/import?${q.toString()}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/octet-stream' },
    body: buf,
  });
}

function showImportDirChoice(info, file, name, pass) {
  openModal('选择命名依据', `
    <div class="notice">服务包里的<b>文件夹名</b>与<b>服务接口域名</b>不一致，请选择以哪一个为准：<br>
      文件夹名：<code>${esc(info.folder)}</code>　服务接口域名：<code>${esc(info.api_domain)}</code></div>
    <div class="field"><label><input type="radio" name="ird" value="domain" checked style="width:auto"> 使用服务接口域名 <code>${esc(info.api_domain)}</code>（重命名文件夹）</label></div>
    <div class="field"><label><input type="radio" name="ird" value="dir" style="width:auto"> 使用文件夹名 <code>${esc(info.folder)}</code>（把服务接口域名改为文件夹名）</label></div>
    <div class="warn-line">选择后仍会校验域名/端口/名称冲突。</div>
  `, async () => {
    const choice = document.querySelector('input[name="ird"]:checked').value;
    try {
      const buf = await file.arrayBuffer();
      await submitImport(buf, name, pass, choice);
      closeModal(); toast('导入完成'); loadServices();
    } catch (e) { $('modalError').textContent = e.message; }
  }, '继续导入');
}

// ---------------- 安全中心 ----------------
async function showSecurity() {
  if (!(await requireVerified('安全中心涉及账户安全操作，请先验证登录密码。'))) return;
  const st = await api('/api/auth/status');
  STATUS = Object.assign(STATUS || {}, st);
  renderSecurity();
}

function securityHtml(st) {
  const pending = PENDING_TOTP ? `
    <div class="panel" style="margin-bottom:14px">
      <h3 style="margin:0 0 8px">添加 TOTP 验证器 · 第 2 步</h3>
      <div class="notice">请把下面的密钥手动添加到验证器 App（Google Authenticator / Microsoft Authenticator 等）：
        <div class="code" style="margin-top:8px">${esc(PENDING_TOTP.secret)}</div>
        <div class="hint" style="margin-top:6px">otpauth：<span class="mono" style="word-break:break-all">${esc(PENDING_TOTP.otpauth_uri)}</span></div>
      </div>
      <div class="field"><label>验证器当前 6 位动态码</label><input id="tpCode" inputmode="numeric" maxlength="6" placeholder="6 位数字"></div>
      <div class="row">
        <button class="btn btn-primary btn-sm" id="btnTotpConfirm">确认绑定</button>
        <button class="btn btn-sm" id="btnTotpCancel">取消</button>
      </div>
    </div>` : '';
  return `
    <div class="panel" style="margin-bottom:14px">
      <h3 style="margin:0 0 8px">当前账户</h3>
      <div class="kv"><span>用户名</span><b>${esc(st.username)}</b></div>
      <div class="kv"><span>密码登录</span><b>${st.totp_enabled ? '已开启 2FA（需动态验证码）' : '未开启 2FA'}</b></div>
      <div class="kv"><span>TOTP 验证器</span><b>${st.totp_count} / 3</b></div>
      <div class="kv"><span>Passkey</span><b>${st.passkey_count} 个（可一键登录）</b></div>
    </div>
    ${pending}
    <div class="panel" style="margin-bottom:14px">
      <h3 style="margin:0 0 8px">修改用户名 / 密码</h3>
      <div class="grid2">
        <div class="field"><label>新用户名（3–32 位）</label><input id="scUser" value="${esc(st.username)}"></div>
        <div class="field"><label>当前密码${st.must_change ? '（首次改密可不填）' : ''}</label><input id="scCur" type="password" autocomplete="current-password"></div>
        <div class="field"><label>新密码（≥8 位）</label><input id="scPass" type="password" autocomplete="new-password"></div>
        <div class="field"><label>再次输入新密码</label><input id="scPass2" type="password" autocomplete="new-password"></div>
      </div>
      <button class="btn btn-primary btn-sm" id="btnSaveCred">保存凭据</button>
    </div>
    <div class="panel" style="margin-bottom:14px">
      <h3 style="margin:0 0 8px">TOTP（最多 3 个；开启后「用户名+密码」登录强制 2FA，Passkey 仍一键登录）</h3>
      ${(st.totp_list || []).length ? `<table><thead><tr><th>名称</th><th>添加时间</th><th>操作</th></tr></thead><tbody>
        ${(st.totp_list || []).map(t => `<tr><td>${esc(t.name)}</td><td class="mono">${tsShort(t.created_at)}</td>
          <td><button class="btn btn-sm btn-danger" data-totp-del="${esc(t.id)}">删除</button></td></tr>`).join('')}
      </tbody></table>` : '<div class="muted">尚未绑定验证器。</div>'}
      <div class="row" style="margin-top:10px">
        <button class="btn btn-sm" id="btnTotpAdd" ${st.totp_count >= 3 ? 'disabled' : ''}>➕ 添加验证器</button>
      </div>
    </div>
    <div class="panel">
      <h3 style="margin:0 0 8px">Passkey（一键登录；可用 Windows Hello / 指纹 / 硬件密钥）</h3>
      ${(st.passkey_list || []).length ? `<table><thead><tr><th>名称</th><th>添加时间</th><th>操作</th></tr></thead><tbody>
        ${(st.passkey_list || []).map(p => `<tr><td>${esc(p.name)}</td><td class="mono">${tsShort(p.created_at)}</td>
          <td><button class="btn btn-sm btn-danger" data-passkey-del="${esc(p.id)}">删除</button></td></tr>`).join('')}
      </tbody></table>` : '<div class="muted">尚未添加 Passkey。</div>'}
      <div class="row" style="margin-top:10px">
        <button class="btn btn-sm" id="btnPasskeyAdd">➕ 添加 Passkey</button>
        <span class="muted" style="font-size:12px">${passkeySupported() ? '' : '⚠ 当前环境不支持 Passkey（需 HTTPS 面板域名访问）'}</span>
      </div>
    </div>`;
}

function tsShort(ts) { return ts ? new Date(ts * 1000).toLocaleString('zh-CN', { hour12: false }) : '—'; }

function renderSecurity() {
  api('/api/auth/account').then(info => {
    const st = Object.assign({}, STATUS, {
      totp_list: info.totp || [],
      passkey_list: info.passkeys || [],
    });
    openModal('🛡️ 安全中心', securityHtml(st), null);
    bindSecurity(st);
  }).catch(e => openModal('安全中心', `<div class="error">${esc(e.message)}</div>`, null));
}

function bindSecurity(st) {
  const save = $('btnSaveCred');
  if (save) save.onclick = async () => {
    const new_username = $('scUser').value.trim();
    const current_password = $('scCur').value;
    const new_password = $('scPass').value;
    const confirm_password = $('scPass2').value;
    if (new_password.length < 8) { $('modalError').textContent = '新密码至少 8 位'; return; }
    if (new_password !== confirm_password) { $('modalError').textContent = '两次输入的新密码不一致'; return; }
    try {
      const r = await api('/api/auth/change-credentials', {
        method: 'POST', body: JSON.stringify({ current_password, new_username, new_password, confirm_password })
      });
      toast(r.message || '凭据已更新');
      STATUS = await api('/api/auth/status');
      showSecurity();
    } catch (e) { $('modalError').textContent = e.message; }
  };

  const add = $('btnTotpAdd');
  if (add) add.onclick = async () => {
    try {
      const b = await api('/api/auth/totp/begin', { method: 'POST' });
      PENDING_TOTP = b; renderSecurity();
    } catch (e) { $('modalError').textContent = e.message; }
  };
  const tc = $('btnTotpConfirm');
  if (tc) tc.onclick = async () => {
    const code = $('tpCode').value.trim();
    if (!code) { $('modalError').textContent = '请输入动态验证码'; return; }
    try {
      const r = await api('/api/auth/totp/confirm', { method: 'POST', body: JSON.stringify({ code, name: '' }) });
      PENDING_TOTP = null; toast(r.message || '已绑定'); showSecurity();
    } catch (e) { $('modalError').textContent = e.message; }
  };
  const tcan = $('btnTotpCancel');
  if (tcan) tcan.onclick = () => { PENDING_TOTP = null; showSecurity(); };

  document.querySelectorAll('[data-totp-del]').forEach(b => b.onclick = async () => {
    if (!confirm('确定删除该验证器？')) return;
    try {
      const r = await api('/api/auth/totp/remove', { method: 'POST', body: JSON.stringify({ id: b.dataset.totpDel }) });
      toast(r.message || '已删除'); showSecurity();
    } catch (e) { $('modalError').textContent = e.message; }
  });

  const pa = $('btnPasskeyAdd');
  if (pa) pa.onclick = async () => {
    if (!passkeySupported()) { $('modalError').textContent = '当前环境不支持 Passkey（需 HTTPS 面板域名访问）'; return; }
    try {
      const b = await api('/api/auth/passkey/register/begin', { method: 'POST' });
      const pk = {
        challenge: b64uDecode(b.challenge),
        rp: b.rp,
        user: { id: b64uDecode(b.user.id), name: b.user.name, displayName: b.user.displayName },
        pubKeyCredParams: b.pubKeyCredParams,
        timeout: b.timeout,
        attestation: b.attestation || 'none',
        authenticatorSelection: b.authenticatorSelection,
      };
      const cred = await navigator.credentials.create({ publicKey: pk });
      const name = prompt('给该 Passkey 起个名字（可留空）') || '';
      const r = await api('/api/auth/passkey/register/finish', {
        method: 'POST',
        body: JSON.stringify({
          id: b64uEncode(cred.rawId),
          name,
          clientDataJSON: b64uEncode(cred.response.clientDataJSON),
          attestationObject: b64uEncode(cred.response.attestationObject),
        }),
      });
      toast(r.message || 'Passkey 已添加'); showSecurity();
    } catch (e) { $('modalError').textContent = e.message || '添加 Passkey 失败'; }
  };
  document.querySelectorAll('[data-passkey-del]').forEach(b => b.onclick = async () => {
    if (!confirm('确定删除该 Passkey？')) return;
    try {
      const r = await api('/api/auth/passkey/remove', { method: 'POST', body: JSON.stringify({ id: b.dataset.passkeyDel }) });
      toast(r.message || '已删除'); showSecurity();
    } catch (e) { $('modalError').textContent = e.message; }
  });
}

// ---------------- 面板设置 ----------------
async function showPanelSettings() {
  const p = await api('/api/panel');
  openModal('面板设置', `
    <div class="notice">修改面板域名/端口后<b>面板自动重启</b>；各中心服务进程不受影响（D38）。重启后需重新登录。
    面板域名用于顶栏显示与 Passkey 的 rpId。</div>
    <div class="field"><label>面板域名（如 acspanel.maxshin.top）</label><input id="sDomain" value="${esc(p.domain)}"></div>
    <div class="field"><label>面板端口</label><input id="sPort" type="number" value="${p.port}"></div>
    <div class="field"><label>监听地址（安全默认：仅本机）</label>
      <input value="${esc(p.bind)}" disabled>
      <div class="hint">如需公网访问请用反向代理，并确保反向代理终止 TLS（Passkey 需要 HTTPS）。</div></div>
  `, async () => {
    const domain = $('sDomain').value.trim();
    const port = Number($('sPort').value);
    if (!domain) { $('modalError').textContent = '域名不能为空'; return; }
    try {
      const r = await api('/api/panel', { method: 'POST', body: JSON.stringify({ domain, port }) });
      closeModal(); toast(r.message || '已保存，面板重启中…');
      setToken(''); STATUS = null;
      setTimeout(() => location.reload(), 3500);
    } catch (e) { $('modalError').textContent = e.message; }
  }, '保存并重启');
}

// ---------------- 日志查看 ----------------
async function showLogs(name) {
  openModal(`服务日志「${name}」`, '<div class="muted">加载中…</div>', null);
  let files = [];
  try {
    const d = await api(`/api/services/${encodeURIComponent(name)}/logs`);
    files = d.files || [];
  } catch (e) {
    $('modalBody').innerHTML = `<div class="error">${esc(e.message)}</div>`;
    return;
  }
  if (!files.length) {
    $('modalBody').innerHTML = '<div class="muted">暂无日志文件（服务尚未启动过？）。</div>';
    return;
  }
  const render = async (file) => {
    $('modalBody').querySelectorAll('.loglist button').forEach(b => b.classList.toggle('on', b.dataset.f === file));
    $('logText').textContent = '加载中…';
    try {
      const d = await api(`/api/services/${encodeURIComponent(name)}/logs/${encodeURIComponent(file)}?lines=500`);
      $('logText').textContent = d.text || '（空）';
      $('logText').scrollTop = $('logText').scrollHeight;
    } catch (e) { $('logText').textContent = '读取失败：' + e.message; }
  };
  $('modalBody').innerHTML = `
    <div class="loglist">${files.map(f =>
      `<button data-f="${esc(f.file)}">${esc(f.file)} <span class="muted">${fmtBytes(f.size)}</span></button>`).join('')}</div>
    <div class="code" id="logText"></div>
    <div class="field" style="margin-top:10px;margin-bottom:0"><button class="btn btn-sm" id="logRefresh">刷新</button></div>`;
  $('modalBody').querySelectorAll('.loglist button').forEach(b => b.onclick = () => render(b.dataset.f));
  $('logRefresh').onclick = () => {
    const on = $('modalBody').querySelector('.loglist button.on');
    if (on) render(on.dataset.f);
  };
  render(files[0].file);
}

// ---------------- 初始化 ----------------
$('btnLogin').onclick = doLogin;
$('lPass').onkeydown = (e) => { if (e.key === 'Enter') doLogin(); };
$('lTotp').onkeydown = (e) => { if (e.key === 'Enter') doLogin(); };
$('btnPasskey').onclick = doPasskeyLogin;
$('btnChange').onclick = doChange;
$('btnChangeLogout').onclick = async () => { try { await api('/api/auth/logout', { method: 'POST' }); } catch (e) {} setToken(''); showAuth(); };
$('btnRefresh').onclick = () => loadServices().catch(e => toast(e.message, true));
$('btnCreate').onclick = showCreate;
$('btnImport').onclick = showImport;
$('btnSecurity').onclick = () => showSecurity().catch(e => toast(e.message, true));
$('btnSettings').onclick = () => showPanelSettings().catch(e => toast(e.message, true));
$('btnLogout').onclick = async () => {
  try { await api('/api/auth/logout', { method: 'POST' }); } catch (e) {}
  setToken(''); STATUS = null; showAuth();
};

(async () => {
  await boot();
  // 运行态自动刷新（10s）
  setInterval(() => {
    if ($('appView').classList.contains('hidden')) return;
    if (!$('modalWrap').classList.contains('hidden')) return;
    loadServices().catch(() => {});
  }, 10000);
})();
