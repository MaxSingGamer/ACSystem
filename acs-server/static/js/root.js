// 根管理员控制台。
document.querySelectorAll('.logo').forEach(e => e.innerHTML = LOGO_BADGE);

async function init() {
  const me = await api('/api/admin/me');
  if (me.role !== 'root') { location.href = '/finance'; return; }
  $('who').textContent = `${me.uid} · 根管理员`;
  await Promise.all([loadOverview(), loadAccounts(), loadAdmins(), loadMembers(), loadKeyStatus(), loadMirrors(), loadRegistry(), loadDaily(7)]);
}
init().catch(e => { if (!TOKEN) location.href = '/login'; else alert(e.message); });

function sec(s) {
  document.querySelectorAll('.section-tab').forEach(t => t.classList.toggle('on', t.dataset.s === s));
  document.querySelectorAll('.section').forEach(v => v.classList.toggle('on', v.id === 's-' + s));
}

/* ---------- 状态 ---------- */
async function loadOverview() {
  const d = await api('/api/stats/overview');
  $('sToday').textContent = fmtA(d.today_flow) + ' A€';
  $('sTodayC').textContent = d.today_count;
  $('sTotal').textContent = fmtA(d.total_flow) + ' A€';
  $('sTotalC').textContent = d.total_count;
  const ac = d.accounts || {};
  $('sAccounts').textContent = ['Country','Bank','Individual','System']
    .map(k => `${k} ${ac[k] ?? 0}`).join(' · ');
}

async function loadDaily(days) {
  const d = await api('/api/stats/daily?days=' + days);
  const items = d.items || [];
  drawLineChart('dailyChart', items.map(x => ({ date: x.date, flow: Number(x.flow) })));
}

/* ---------- 账户 ---------- */
async function loadAccounts() {
  const atype = $('accType').value, q = $('accSearch').value.trim();
  const d = await api(`/api/accounts?atype=${atype}&search=${encodeURIComponent(q)}`);
  const rows = d.items || [];
  $('accBody').innerHTML = rows.map(a => `
    <tr>
      <td class="mono">${esc(a.uid)}</td>
      <td>${esc(a.email || '—')}</td>
      <td class="mono">${fmtA(a.balance)} A€</td>
      <td><span class="tag ${a.status.toLowerCase()}">${esc(a.status)}</span></td>
      <td class="ops">
        ${a.status === 'Active' ? `<button class="btn-sm btn-secondary" onclick="accAction('${atype}','${esc(a.uid)}','freeze')">冻结</button>` : ''}
        ${a.status === 'Frozen' ? `<button class="btn-sm btn-secondary" onclick="accAction('${atype}','${esc(a.uid)}','unfreeze')">解冻</button>` : ''}
        ${a.status !== 'Deleted' ? `<button class="btn-sm btn-danger" onclick="accAction('${atype}','${esc(a.uid)}','delete')">注销</button>` : ''}
      </td>
    </tr>`).join('') || '<tr><td colspan="5" class="muted">无记录</td></tr>';
}

async function accAction(atype, uid, act) {
  if (act === 'delete' && !confirm(`确定注销账户 ${uid}？（状态改为 Deleted，账本只读保留供审计，不可逆）`)) return;
  const url = `/api/accounts/${atype}/${encodeURIComponent(uid)}` + (act === 'delete' ? '' : '/' + act);
  await api(url, { method: act === 'delete' ? 'DELETE' : 'POST' });
  loadAccounts();
}

async function loadAdmins() {
  const d = await api('/api/admins');
  const rows = d.items || [];
  $('adminBody').innerHTML = rows.map(a => `
    <tr>
      <td class="mono">${esc(a.uid)}</td>
      <td>${a.role === 'root' ? '根管理员' : '金融部'}</td>
      <td><span class="tag ${a.status.toLowerCase()}">${esc(a.status)}</span>${a.must_change_password ? ' <span class="tag pending">待改密</span>' : ''}</td>
      <td class="ops">
        <button class="btn-sm btn-secondary" onclick="showChg('${esc(a.uid)}')">重置密码</button>
        ${a.status === 'Active' ? `<button class="btn-sm btn-secondary" onclick="adminToggle(${a.id},'disable')">停用</button>` : ''}
        ${a.status === 'Disabled' ? `<button class="btn-sm btn-secondary" onclick="adminToggle(${a.id},'enable')">启用</button>` : ''}
      </td>
    </tr>`).join('') || '<tr><td colspan="4" class="muted">无记录</td></tr>';
}

async function createAdmin() {
  const uid = $('aUid').value.trim(), pwd = $('aPwd').value, role = $('aRole').value;
  if (!uid || pwd.length < 8) { alert('请填写 UID 与至少 8 位初始密码'); return; }
  await api('/api/admins', { method: 'POST', body: JSON.stringify({ uid, password: pwd, role }) });
  $('aUid').value = ''; $('aPwd').value = '';
  loadAdmins();
}

async function adminToggle(id, act) { await api(`/api/admins/${id}/${act}`, { method: 'POST' }); loadAdmins(); }

/* ---------- 成员国家 / 银行认定 ---------- */
async function loadMembers() {
  await loadMemberList('countries', 'countryBody');
  await loadMemberList('companies', 'bankBody');
}

async function loadMemberList(kind, tbody) {
  if (!$(tbody)) return;
  const d = await api(`/api/members/${kind}`);
  const rows = d.items || [];
  $(tbody).innerHTML = rows.map(m => `
    <tr>
      <td class="mono">${m.id}</td>
      <td>${esc(m.name)}</td>
      <td><span class="tag ${m.status.toLowerCase()}">${esc(m.status)}</span></td>
      <td class="ops">
        ${m.status === 'Active' ? `<button class="btn-sm btn-secondary" onclick="memToggle('${kind}',${m.id},'Deleted')">取消登记</button>` : `<button class="btn-sm btn-secondary" onclick="memToggle('${kind}',${m.id},'Active')">重新登记</button>`}
        <button class="btn-sm btn-danger" onclick="memDel('${kind}',${m.id})">删除</button>
      </td>
    </tr>`).join('') || '<tr><td colspan="4" class="muted">无记录</td></tr>';
}

async function createMember(kind) {
  const nameEl = kind === 'countries' ? $('countryName') : $('bankName');
  const name = nameEl.value.trim();
  if (!name) { alert('请输入名称'); return; }
  await api(`/api/members/${kind}`, { method: 'POST', body: JSON.stringify({ name, status: 'Active' }) });
  nameEl.value = '';
  loadMemberList(kind, kind === 'countries' ? 'countryBody' : 'bankBody');
}

async function memToggle(kind, id, status) {
  await api(`/api/members/${kind}/${id}`, { method: 'PUT', body: JSON.stringify({ status }) });
  loadMemberList(kind, kind === 'countries' ? 'countryBody' : 'bankBody');
}
async function memDel(kind, id) {
  if (!confirm('确定删除该成员记录？')) return;
  await api(`/api/members/${kind}/${id}`, { method: 'DELETE' });
  loadMemberList(kind, kind === 'countries' ? 'countryBody' : 'bankBody');
}

/* ---------- 安全：铸造 + 密钥 ---------- */
async function loadKeyStatus() {
  const d = await api('/api/admin/keys/status');
  $('keyStatus').innerHTML = `
    <div class="kv-row"><span>密钥</span><b>${d.exists ? '已生成' : '未生成'}</b></div>
    <div class="kv-row"><span>解锁状态</span><b><span class="tag ${d.unlocked ? 'active' : 'locked'}">${d.unlocked ? '已解锁（可签名铸造）' : '已锁定'}</span></b></div>`;
}

async function unlockKey() {
  const pwd = $('keyPwd').value;
  if (!pwd) { alert('请输入登录密码'); return; }
  await api('/api/admin/keys/unlock', { method: 'POST', body: JSON.stringify({ password: pwd }) });
  $('keyPwd').value = ''; $('safeMsg').textContent = '密钥已解锁'; loadKeyStatus();
}
async function lockKey() { await api('/api/admin/keys/lock', { method: 'POST' }); $('safeMsg').textContent = '密钥已锁定'; loadKeyStatus(); }
async function exportKey() {
  const pwd = $('keyPwd').value;
  if (!pwd) { alert('请输入登录密码以导出密钥'); return; }
  const d = await api('/api/admin/keys/export', { method: 'POST', body: JSON.stringify({ password: pwd }) });
  $('safeMsg').textContent = `已导出到 ${d.path}`;
}

async function doMint() {
  const amt = Number($('mintAmt').value);
  if (!amt || amt <= 0) { alert('请输入有效金额'); return; }
  await api('/api/admin/mint', { method: 'POST', body: JSON.stringify({ amount: amt }) });
  $('safeMsg').textContent = `铸造 ${amt} A€ 完成`;
  loadOverview(); loadAccounts();
}

/* ---------- 镜像 ---------- */
async function loadMirrors() {
  const d = await api('/api/admin/mirror-keys');
  const rows = d.items || [];
  $('mirBody').innerHTML = rows.map(k => `
    <tr>
      <td>${esc(k.name)}</td>
      <td class="mono" style="font-size:12px">${esc(k.apikey)}</td>
      <td><span class="tag ${k.status.toLowerCase()}">${esc(k.status)}</span></td>
      <td class="mono">${k.last_pull_at ? tsFmt(k.last_pull_at) : '—'}</td>
      <td class="ops"><button class="btn-sm btn-danger" onclick="delMirror('${esc(k.apikey)}')">删除</button></td>
    </tr>`).join('') || '<tr><td colspan="5" class="muted">无记录</td></tr>';
}
async function addMirror() {
  const name = $('mirName').value.trim();
  if (!name) { alert('请输入镜像名称'); return; }
  await api('/api/admin/mirror-keys', { method: 'POST', body: JSON.stringify({ name }) });
  $('mirName').value = ''; loadMirrors();
}
async function delMirror(apikey) { if (!confirm('删除该镜像 apikey？')) return; await api(`/api/admin/mirror-keys/${encodeURIComponent(apikey)}`, { method: 'DELETE' }); loadMirrors(); }

/* ---------- 社区镜像注册 ---------- */
async function loadRegistry() {
  const d = await api('/api/admin/mirror-registry');
  $('regBody').innerHTML = (d.items || []).map(k => `
    <tr>
      <td>${esc(k.url)}</td>
      <td>${esc(k.name) || '-'}</td>
      <td>${esc(k.note) || '-'}</td>
      <td>${k.status === 'Active' ? '启用' : '停用'}</td>
      <td class="ops"><button class="btn-sm btn-danger" onclick="delRegistry('${esc(k.url)}')">删除</button></td>
    </tr>`).join('') || '<tr><td colspan="5" class="muted">暂无社区镜像</td></tr>';
}
async function addRegistry() {
  const url = $('regUrl').value.trim();
  if (!url) { alert('请输入镜像地址'); return; }
  await api('/api/admin/mirror-registry', { method: 'POST', body: JSON.stringify({ url, name: $('regName').value, note: $('regNote').value }) });
  $('regUrl').value = ''; $('regName').value = ''; $('regNote').value = '';
  loadRegistry();
}
async function delRegistry(url) {
  if (!confirm('删除该社区镜像？')) return;
  await api(`/api/admin/mirror-registry/${encodeURIComponent(url)}`, { method: 'DELETE' });
  loadRegistry();
}

/* ---------- 审计 ---------- */
async function unlockAudit() {
  const pwd = $('audPwd').value;
  if (!pwd) { alert('请输入密码'); return; }
  await api('/api/audit/unlock', { method: 'POST', body: JSON.stringify({ password: pwd }) });
  $('audLockPanel').classList.add('hidden');
  $('audContent').classList.remove('hidden');
  $('billPanel').classList.remove('hidden');
  loadAudit(); loadBills();
}

async function loadAudit() {
  const d = await api('/api/audit?limit=50');
  const rows = d.items || [];
  $('audBody').innerHTML = rows.map(e => `
    <tr><td>${e.id}</td><td class="mono">${esc(e.actor || 'system')}</td>
      <td>${esc(e.op)}</td><td style="font-size:12px">${esc(e.detail || '')}</td>
      <td class="mono">${tsFmt(e.ts)}</td></tr>`).join('') || '<tr><td colspan="5" class="muted">无记录</td></tr>';
}

async function exportLog() {
  const f = $('expFrom').value.trim(), t = $('expTo').value.trim();
  const qs = new URLSearchParams();
  if (f) qs.set('from', f);
  if (t) qs.set('to', t);
  const text = await api('/api/audit/export?' + qs.toString(), { raw: true });
  const blob = new Blob([text], { type: 'text/plain' });
  const a = document.createElement('a');
  a.href = URL.createObjectURL(blob); a.download = `audit_${f || 'all'}_${t || 'now'}.log`; a.click();
}

async function loadBills() {
  const dt = $('billDate').value.trim() || '';
  const d = await api('/api/stats/bills' + (dt ? `?date=${dt}` : ''));
  const rows = d.items || [];
  $('billBody').innerHTML = rows.map(b => `
    <tr>
      <td class="mono">${tsFmt(b.timestamp)}</td>
      <td>${esc(b.tx_type)}</td>
      <td class="mono">${esc(b.sender)} <span class="muted">· ${typeName(b.sender_type)}</span></td>
      <td class="mono">${esc(b.receiver)} <span class="muted">· ${typeName(b.receiver_type)}</span></td>
      <td class="mono">${fmtA(b.amount)} A€</td>
      <td><span class="tag ${b.status.toLowerCase()}">${esc(b.status)}</span></td>
      <td class="mono" style="font-size:11px">${b.central_signed ? '✓ 已签名' : '—'}</td>
    </tr>`).join('') || '<tr><td colspan="7" class="muted">无记录</td></tr>';
}

/* ---------- 改密 ---------- */
function showChg(target) { $('cTarget').value = target || ''; $('cMsg').textContent = ''; $('chgPanel').classList.remove('hidden'); }

async function doChg() {
  const old = $('cOld').value, np = $('cNew').value, tgt = $('cTarget').value.trim();
  if (np.length < 8) { $('cMsg').textContent = '新密码至少 8 位'; return; }
  await api('/api/admin/change-password', { method: 'POST', body: JSON.stringify({
    old_password: old || undefined, new_password: np, target_uid: tgt || undefined }) });
  $('cMsg').textContent = '修改成功' + (tgt ? '' : '，请重新登录');
  if (!tgt) { setTimeout(() => location.href = '/login', 800); }
  else { $('chgPanel').classList.add('hidden'); loadAdmins(); }
}

async function logout() {
  try { await api('/api/admin/logout', { method: 'POST' }); } catch (e) {}
  localStorage.removeItem('acs_token'); localStorage.removeItem('acs_aud_unlocked');
  location.href = '/login';
}
