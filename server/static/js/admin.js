// 管理员（Admin，原「金融部」Finance）控制台。
// v3.3.0（D18）：角色由 finance 更名为 admin；账号由 Root 创建/停用/重置密码。
document.querySelectorAll('.logo').forEach(e => e.innerHTML = LOGO_BADGE);

async function init() {
  const me = await api('/api/admin/me');
  if (me.role !== 'admin') { location.href = '/root'; return; }
  $('who').textContent = `${me.uid} · 管理员`;
  // v3.3.0（D15）：Admin 不持有主密钥副本，只能显示是否已由根管理员解锁
  const mk = $('mkState'); if (mk) mk.textContent = me.master_key_unlocked ? '🔓 主密钥已解锁' : '🔒 主密钥未解锁（需根管理员登录）';
  await Promise.all([loadOverview(), loadAccounts(), loadDaily(7)]);
}
init().catch(e => { if (!TOKEN) location.href = '/login'; else alert(e.message); });

function sec(s) {
  document.querySelectorAll('.section-tab').forEach(t => t.classList.toggle('on', t.dataset.s === s));
  document.querySelectorAll('.section').forEach(v => v.classList.toggle('on', v.id === 's-' + s));
  // v3.3.0 D14：审计页不再有"解锁"这一步，切过去即直接加载
  if (s === 'aud') loadAuditTab();
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

/* ---------- 账本账户（v3.3.0 D14/D21：可查/冻结/解冻全部类型，但不可注册或删除） ---------- */
function currentAccType() {
  const sel = $('accType');
  return sel ? sel.value : 'Individual';
}
async function loadAccounts() {
  const q = $('accSearch').value.trim();
  const atype = currentAccType();
  const d = await api(`/api/accounts?atype=${encodeURIComponent(atype)}&search=${encodeURIComponent(q)}`);
  const rows = d.items || [];
  $('accBody').innerHTML = rows.map(a => `
    <tr>
      <td class="mono">${esc(a.uid)}</td>
      <td>${esc(a.fullname || '—')}</td>
      <td>${esc(a.email || '—')}</td>
      <td class="mono">${fmtA(a.balance)} A€</td>
      <td><span class="tag ${a.status.toLowerCase()}">${esc(a.status)}</span></td>
      <td class="ops">
        ${a.status === 'Active' ? `<button class="btn-sm btn-secondary" onclick="accAction('${esc(a.uid)}','freeze')">冻结</button>` : ''}
        ${a.status === 'Frozen' ? `<button class="btn-sm btn-secondary" onclick="accAction('${esc(a.uid)}','unfreeze')">解冻</button>` : ''}
      </td>
    </tr>`).join('') || '<tr><td colspan="6" class="muted">无记录</td></tr>';
}
async function accAction(uid, act) {
  const atype = currentAccType();
  await api(`/api/accounts/${encodeURIComponent(atype)}/${encodeURIComponent(uid)}/${act}`, { method: 'POST' });
  loadAccounts();
}

/* ---------- 注（v3.3.0 #2）：成员企业认定已取消 ---------- */
/* 企业账户现在与个人账户一样，凭同意协议即可由客户端自助注册，无需金融部认定。 */

/* ---------- 审计（v3.3.0 D14：已取消二次密码校验，登录即可查看） ---------- */
function loadAuditTab() { loadBills(); }

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
