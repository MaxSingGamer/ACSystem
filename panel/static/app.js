// AlphaCoin 服务中心面板前端（v3.3.0 #13）
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
  toast._t = setTimeout(() => t.classList.add('hidden'), isErr ? 5200 : 2800);
}

async function api(path, opts = {}) {
  const r = await fetch(path, Object.assign({ headers: { 'Content-Type': 'application/json' } }, opts));
  let data = null;
  const ct = r.headers.get('content-type') || '';
  if (ct.includes('application/json')) {
    try { data = await r.json(); } catch (e) { data = null; }
  }
  if (!r.ok) {
    const msg = (data && data.error) ? data.error : ('HTTP ' + r.status);
    throw new Error(msg);
  }
  return data;
}

// ---------------- 弹窗 ----------------
function openModal(title, bodyHtml, onOk, okText) {
  $('modalTitle').textContent = title;
  $('modalBody').innerHTML = bodyHtml;
  $('modalError').textContent = '';
  $('modalWrap').classList.remove('hidden');
  $('modalOk').textContent = okText || '确定';
  $('modalOk').classList.toggle('hidden', !onOk);
  $('modalCancel').textContent = onOk ? '取消' : '关闭';
  $('modalOk').onclick = onOk || null;
  $('modalCancel').onclick = closeModal;
  $('modalClose').onclick = closeModal;
}
function closeModal() { $('modalWrap').classList.add('hidden'); $('modalBody').innerHTML = ''; }

// ---------------- 首次配置向导 ----------------
async function checkSetup() {
  const p = await api('/api/panel');
  $('panelAddr').textContent = `http://${p.bind}:${p.port}`;
  $('statPanelPort').textContent = p.port;
  $('statRoot').textContent = p.root || '–';
  if (!p.configured) { showSetup(p); return false; }
  return true;
}
function showSetup(p) {
  openModal('首次使用 · 配置面板域名与端口', `
    <div class="notice">首次启动使用默认端口 <b>9600</b>。请填写面板域名（仅作记录，不参与网络绑定）与端口。
    保存后面板会<b>自动重启</b>——各中心服务不受影响。</div>
    <div class="field"><label>面板域名（仅记录，如 acpanel.maxshin.top）</label>
      <input id="pDomain" placeholder="acpanel.maxshin.top" value="${esc(p.domain || '')}"></div>
    <div class="field"><label>面板端口（默认 9600）</label>
      <input id="pPort" type="number" min="1" max="65535" value="${esc(p.port || 9600)}">
      <div class="hint">注意：该端口不能与任何服务的 API/管理端口冲突。</div></div>
  `, async () => {
    const domain = $('pDomain').value.trim();
    const port = Number($('pPort').value);
    if (!domain) { $('modalError').textContent = '请填写面板域名'; return; }
    if (!port || port < 1 || port > 65535) { $('modalError').textContent = '端口无效'; return; }
    try {
      const r = await api('/api/panel', { method: 'POST', body: JSON.stringify({ domain, port }) });
      $('modalError').textContent = '';
      toast(r.message || '配置已保存，面板正在重启…');
      closeModal();
      // 等待重启完成后自动刷新
      setTimeout(() => location.reload(), 3500);
    } catch (e) { $('modalError').textContent = e.message; }
  }, '保存并重启面板');
}

// ---------------- 服务列表 ----------------
let SERVICES = [];
async function loadServices() {
  const d = await api('/api/services');
  SERVICES = d.services || [];
  $('statCount').textContent = SERVICES.length;
  $('statRunning').textContent = SERVICES.filter(s => s.state === 'running').length;
  $('panelAddr').textContent = `http://${d.panel.bind}:${d.panel.port}`;
  $('statPanelPort').textContent = d.panel.port;
  const body = $('svcBody');
  if (!SERVICES.length) {
    body.innerHTML = '<tr><td colspan="6" class="muted">还没有服务：点右上角「新建服务」，或「导入服务包」。</td></tr>';
    return;
  }
  body.innerHTML = SERVICES.map(s => `
    <tr>
      <td><b>${esc(s.name)}</b></td>
      <td class="mono">${esc(s.domain)}</td>
      <td class="mono">${s.api_port}</td>
      <td class="mono">${s.admin_port}</td>
      <td><span class="tag ${esc(s.state)}">${stateText(s.state)}</span></td>
      <td><div class="ops">
        ${s.state === 'running'
          ? `<button class="btn btn-sm" data-act="stop" data-n="${esc(s.name)}">停止</button>`
          : `<button class="btn btn-sm btn-primary" data-act="start" data-n="${esc(s.name)}">启动</button>`}
        <button class="btn btn-sm" data-act="logs" data-n="${esc(s.name)}">日志</button>
        <button class="btn btn-sm" data-act="edit" data-n="${esc(s.name)}">改域名/端口</button>
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
      <div class="field"><label>域名（唯一）</label><input id="nDomain" placeholder="如 acsystem.maxshin.top"></div>
      <div class="field"><label>品牌名（可选）</label><input id="nBrand" placeholder="Alpha Coin"></div>
      <div class="field"><label>API 端口（无默认值，必填）</label><input id="nApi" type="number" placeholder="如 9700"></div>
      <div class="field"><label>管理端口（无默认值，必填）</label><input id="nAdmin" type="number" placeholder="如 9701"></div>
    </div>
    <div class="field"><label>根管理员账户（每行一个 <code>uid:密码</code>，至少一个；首启后不可增减）</label>
      <textarea id="nRoots" placeholder="Max_Shin:ChangeMe123!"></textarea></div>
    <div class="field"><label>系统账户（每行一个 <code>uid:密码</code>，可留空）</label>
      <textarea id="nSystems" placeholder="PreIssuedAccount:ChangeMe123!&#10;AESystem:ChangeMe123!"></textarea></div>
    <div class="field"><label>发行账户 uid（填了系统账户则必填，须为其中之一）</label>
      <input id="nPre" placeholder="PreIssuedAccount"></div>
    <div class="warn-line">端口必须与其他服务及面板端口不同；新建后只允许修改域名与端口。</div>
  `, async () => {
    const roots = splitLines($('nRoots').value);
    const systems = splitLines($('nSystems').value);
    const req = {
      name: $('nName').value.trim(),
      domain: $('nDomain').value.trim(),
      api_port: Number($('nApi').value),
      admin_port: Number($('nAdmin').value),
      root_accounts: roots,
      system_accounts: systems,
      pre_issued: $('nPre').value.trim(),
      brand_name: $('nBrand').value.trim(),
      brand_currency: '',
    };
    if (!req.name || !req.domain || !req.api_port || !req.admin_port) {
      $('modalError').textContent = '名称、域名、API 端口、管理端口均为必填'; return;
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

// ---------------- 改域名/端口 ----------------
function showEdit(svc) {
  openModal(`修改服务「${svc.name}」`, `
    <div class="notice">仅允许修改<b>域名</b>与<b>端口</b>。改域名会同步：服务目录名 → servers.json → 该服务 .env。</div>
    <div class="field"><label>域名</label><input id="eDomain" value="${esc(svc.domain)}"></div>
    <div class="grid2">
      <div class="field"><label>API 端口</label><input id="eApi" type="number" value="${svc.api_port}"></div>
      <div class="field"><label>管理端口</label><input id="eAdmin" type="number" value="${svc.admin_port}"></div>
    </div>
    ${svc.state === 'running' ? '<div class="warn-line">服务正在运行：建议先停止再改，否则需重启服务后生效。</div>' : ''}
  `, async () => {
    const req = {
      domain: $('eDomain').value.trim(),
      api_port: Number($('eApi').value),
      admin_port: Number($('eAdmin').value),
    };
    try {
      await api(`/api/services/${encodeURIComponent(svc.name)}`, { method: 'PUT', body: JSON.stringify(req) });
      closeModal(); toast('已更新'); loadServices();
    } catch (e) { $('modalError').textContent = e.message; }
  }, '保存');
}

// ---------------- 导出服务包 ----------------
function showExport(svc) {
  openModal(`导出 AlphaTar 服务包「${svc.name}」`, `
    <div class="notice">导出的是<b>整个服务数据目录</b>（含数据库 / .env / 日志），文件名：
      <code>${esc(svc.domain)}.gz.alphatar</code>。服务必须先停止，保证数据库快照一致。</div>
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
    <div class="notice">包内域名 / 端口会自动读取；<b>服务名称不打包</b>，需在此设置（保证唯一）。
    若名称 / 域名 / 端口与现有服务或面板冲突，会提示修改后再导入。</div>
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
      const q = new URLSearchParams({ name, password: pass });
      const r = await api(`/api/import?${q.toString()}`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/octet-stream' },
        body: buf,
      });
      closeModal();
      toast(r.message || '导入完成');
      loadServices();
    } catch (e) { $('modalError').textContent = e.message; }
  }, '导入');
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

// ---------------- 面板设置 ----------------
async function showPanelSettings() {
  const p = await api('/api/panel');
  openModal('面板设置', `
    <div class="notice">修改面板域名/端口后<b>面板自动重启</b>；各中心服务进程不受影响（D38）。
    面板域名仅作记录，不参与网络绑定。</div>
    <div class="field"><label>面板域名</label><input id="sDomain" value="${esc(p.domain)}"></div>
    <div class="field"><label>面板端口</label><input id="sPort" type="number" value="${p.port}"></div>
    <div class="field"><label>监听地址（安全默认：仅本机）</label>
      <input value="${esc(p.bind)}" disabled>
      <div class="hint">如需公网访问请用反向代理，并自行加认证——面板可启停/删除服务。</div></div>
  `, async () => {
    const domain = $('sDomain').value.trim();
    const port = Number($('sPort').value);
    if (!domain) { $('modalError').textContent = '域名不能为空'; return; }
    try {
      const r = await api('/api/panel', { method: 'POST', body: JSON.stringify({ domain, port }) });
      closeModal(); toast(r.message || '已保存，面板重启中…');
      setTimeout(() => location.reload(), 3500);
    } catch (e) { $('modalError').textContent = e.message; }
  }, '保存并重启');
}

// ---------------- 初始化 ----------------
$('btnRefresh').onclick = () => loadServices().catch(e => toast(e.message, true));
$('btnCreate').onclick = showCreate;
$('btnImport').onclick = showImport;
$('btnSettings').onclick = () => showPanelSettings().catch(e => toast(e.message, true));

(async () => {
  try {
    const ok = await checkSetup();
    if (ok) await loadServices();
  } catch (e) {
    toast('面板加载失败：' + e.message, true);
  }
  // 运行态自动刷新（10s）
  setInterval(() => { if (!$('modalWrap').classList.contains('hidden')) return; loadServices().catch(() => {}); }, 10000);
})();
