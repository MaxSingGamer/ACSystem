// 根管理员控制台。
document.querySelectorAll('.logo').forEach(e => e.innerHTML = LOGO_BADGE);

async function init() {
  const me = await api('/api/admin/me');
  if (me.role !== 'root') { location.href = '/admin'; return; }
  $('who').textContent = `${me.uid} · 根管理员`;
  // v3.3.0（D15）：主密钥解锁状态（根管理员登录即解锁）
  const mk = $('mkState'); if (mk) mk.textContent = me.master_key_unlocked ? '🔓 主密钥已解锁' : '🔒 主密钥未解锁';
  await Promise.all([loadOverview(), loadAccounts(), loadAdmins(), loadKeyStatus(), loadDaily(7)]);
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
  const ac = d.accounts || {};
  $('sAccounts').textContent = ['Country','Company','Individual','System']
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
  // v3.3.0（#4/#5）：展示「全称（简称）」；国家 / 企业有全称，个人 / 系统为空
  $('accBody').innerHTML = rows.map(a => `
    <tr>
      <td class="mono">${esc(a.uid)}</td>
      <td>${esc(a.fullname || '—')}</td>
      <td>${esc(a.email || '—')}</td>
      <td class="mono">${fmtA(a.balance)} A€</td>
      <td><span class="tag ${a.status.toLowerCase()}">${esc(a.status)}</span></td>
      <td class="ops">
        ${a.status === 'Active' ? `<button class="btn-sm btn-secondary" onclick="accAction('${atype}','${esc(a.uid)}','freeze')">冻结</button>` : ''}
        ${a.status === 'Frozen' ? `<button class="btn-sm btn-secondary" onclick="accAction('${atype}','${esc(a.uid)}','unfreeze')">解冻</button>` : ''}
      </td>
    </tr>`).join('') || '<tr><td colspan="6" class="muted">无记录</td></tr>';
}

async function accAction(atype, uid, act) {
  // v3.3.0（D21）：后台不再提供「注册 / 注销」账本账户；仅冻结 / 解冻
  await api(`/api/accounts/${atype}/${encodeURIComponent(uid)}/${act}`, { method: 'POST' });
  loadAccounts();
}

async function loadAdmins() {
  const d = await api('/api/admins');
  const rows = d.items || [];
  $('adminBody').innerHTML = rows.map(a => `
    <tr>
      <td class="mono">${esc(a.uid)}</td>
      <td>${a.role === 'root' ? '根管理员' : '管理员'}</td>
      <td><span class="tag ${a.status.toLowerCase()}">${esc(a.status)}</span>${a.must_change_password ? ' <span class="tag pending">待改密</span>' : ''}</td>
      <td class="ops">
        ${a.role === 'root' ? '<span class="muted">—</span>' : `
          <button class="btn-sm btn-secondary" onclick="showChg('${esc(a.uid)}')">重置密码</button>
          ${a.status === 'Active' ? `<button class="btn-sm btn-secondary" onclick="adminToggle(${a.id},'disable')">停用</button>` : ''}
          ${a.status === 'Disabled' ? `<button class="btn-sm btn-secondary" onclick="adminToggle(${a.id},'enable')">启用</button>` : ''}`}
      </td>
    </tr>`).join('') || '<tr><td colspan="4" class="muted">无记录</td></tr>';
}

async function createAdmin() {
  // v3.3.0（D17/D18）：此处只能创建「管理员（Admin）」；Root 账户首启后不可增减。
  const uid = $('aUid').value.trim(), pwd = $('aPwd').value;
  if (!uid || pwd.length < 8) { alert('请填写 UID 与至少 8 位初始密码'); return; }
  await api('/api/admins', { method: 'POST', body: JSON.stringify({ uid, password: pwd }) });
  $('aUid').value = ''; $('aPwd').value = '';
  loadAdmins();
}

async function adminToggle(id, act) { await api(`/api/admins/${id}/${act}`, { method: 'POST' }); loadAdmins(); }

/* ---------- 注（v3.3.0 #2）：成员国家 / 企业认定已取消 ---------- */
/* 国家 / 企业账户现在与个人账户一样，凭同意协议即可由客户端自助注册，无需理事会认定。 */

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

/* ---------- 审计（v3.3.0 D14：已取消二次密码校验，登录即可查看） ---------- */
/// 切到审计页时直接加载（不再要求输密码）。
function loadAuditTab() {
  const ac = $('audContent'), bp = $('billPanel');
  if (ac) ac.classList.remove('hidden');
  if (bp) bp.classList.remove('hidden');
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
  localStorage.removeItem('acs_token');
  location.href = '/login';
}
