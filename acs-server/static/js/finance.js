// 金融部控制台。
document.querySelectorAll('.logo').forEach(e => e.innerHTML = LOGO_BADGE);

async function init() {
  const me = await api('/api/admin/me');
  if (me.role !== 'finance') { location.href = '/root'; return; }
  $('who').textContent = `${me.uid} · 金融部`;
  await Promise.all([loadOverview(), loadAccounts(), loadCompanies(), loadDaily(7)]);
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
}

async function loadDaily(days) {
  const d = await api('/api/stats/daily?days=' + days);
  const items = d.items || [];
  drawLineChart('dailyChart', items.map(x => ({ date: x.date, flow: Number(x.flow) })));
}

/* ---------- 银行账户 ---------- */
async function loadAccounts() {
  const q = $('accSearch').value.trim();
  const d = await api(`/api/accounts?atype=Bank&search=${encodeURIComponent(q)}`);
  const rows = d.items || [];
  $('accBody').innerHTML = rows.map(a => `
    <tr>
      <td class="mono">${esc(a.uid)}</td>
      <td>${esc(a.email || '—')}</td>
      <td class="mono">${fmtA(a.balance)} A€</td>
      <td><span class="tag ${a.status.toLowerCase()}">${esc(a.status)}</span></td>
      <td class="ops">
        ${a.status === 'Active' ? `<button class="btn-sm btn-secondary" onclick="accAction('${esc(a.uid)}','freeze')">冻结</button>` : ''}
        ${a.status === 'Frozen' ? `<button class="btn-sm btn-secondary" onclick="accAction('${esc(a.uid)}','unfreeze')">解冻</button>` : ''}
      </td>
    </tr>`).join('') || '<tr><td colspan="5" class="muted">无记录</td></tr>';
}
async function accAction(uid, act) {
  await api(`/api/accounts/Bank/${encodeURIComponent(uid)}/${act}`, { method: 'POST' });
  loadAccounts();
}

/* ---------- 成员企业 ---------- */
async function loadCompanies() {
  const d = await api('/api/members/companies');
  const rows = d.items || [];
  $('memBody').innerHTML = rows.map(c => `
    <tr>
      <td class="mono">${c.id}</td>
      <td>${esc(c.name)}</td>
      <td><span class="tag ${c.status.toLowerCase()}">${esc(c.status)}</span></td>
      <td class="ops">
        ${c.status === 'Active' ? `<button class="btn-sm btn-secondary" onclick="memToggle(${c.id},'Inactive')">撤销认定</button>` : `<button class="btn-sm btn-secondary" onclick="memToggle(${c.id},'Active')">重新认定</button>`}
        <button class="btn-sm btn-danger" onclick="memDel(${c.id})">删除</button>
      </td>
    </tr>`).join('') || '<tr><td colspan="4" class="muted">无记录</td></tr>';
}
async function addCompany() {
  const name = $('mName').value.trim();
  if (!name) { alert('请填写企业名称'); return; }
  await api('/api/members/companies', { method: 'POST', body: JSON.stringify({ name }) });
  $('mName').value = '';
  loadCompanies();
}
async function memToggle(id, status) {
  await api(`/api/members/companies/${id}`, { method: 'PUT', body: JSON.stringify({ status }) });
  loadCompanies();
}
async function memDel(id) {
  if (!confirm('确定删除该成员记录？')) return;
  await api(`/api/members/companies/${id}`, { method: 'DELETE' });
  loadCompanies();
}

/* ---------- 审计 ---------- */
async function unlockAudit() {
  const pwd = $('audPwd').value;
  if (!pwd) { alert('请输入密码'); return; }
  await api('/api/audit/unlock', { method: 'POST', body: JSON.stringify({ password: pwd }) });
  $('audLockPanel').classList.add('hidden');
  $('billPanel').classList.remove('hidden');
  loadBills();
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
function showChg() { $('cMsg').textContent = ''; $('chgPanel').classList.remove('hidden'); }
async function doChg() {
  const old = $('cOld').value, np = $('cNew').value;
  if (np.length < 8) { $('cMsg').textContent = '新密码至少 8 位'; return; }
  await api('/api/admin/change-password', { method: 'POST', body: JSON.stringify({ old_password: old, new_password: np }) });
  $('cMsg').textContent = '修改成功，请重新登录';
  setTimeout(() => location.href = '/login', 800);
}

async function logout() {
  try { await api('/api/admin/logout', { method: 'POST' }); } catch (e) {}
  localStorage.removeItem('acs_token');
  location.href = '/login';
}
