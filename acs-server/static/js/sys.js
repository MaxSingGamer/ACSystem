// 系统账本账户登录 / 钱包操作（后台代管系统账户，功能与 Alpha Wallet 客户端一致）。
// 复用 common.js 的 api()/TOKEN/$/esc/fmt/typeName/tsFmt 与主题。
const NAV = [['overview', '概览'], ['send', '转账'], ['inbox', '待收箱'], ['txs', '流水']];
let view = 'overview';
let acting = null;
let st = null;

async function initSys() {
  // 未登录则回登录页
  if (!TOKEN) { location.href = '/login'; return; }
  try {
    const me = await api('/api/admin/me');
    $('who').textContent = (me.uid || '') + ' · ' + (me.role === 'root' ? '根管理员' : '金融部');
  } catch (e) { location.href = '/login'; return; }
  loadList();
}

async function loadList() {
  try {
    const j = await api('/api/admin/sys/list');
    const items = j.items || [];
    const el = $('sysList');
    if (!items.length) {
      el.innerHTML = '<p class="muted">暂无系统账户（请在服务端 .env 配置 ACS_SYSTEM_ACCOUNTS 后重启种子创建）。</p>';
      return;
    }
    el.innerHTML = items.map(x =>
      `<div class="sys-card" data-u="${esc(x.uid)}"><div class="nm">${esc(x.uid)}</div>
        <div class="muted" style="margin:6px 0">${esc(x.email)}</div>
        <div style="font-size:24px;font-weight:800">${fmt(x.balance)} <span class="muted" style="font-size:13px">A€</span></div>
        <div style="margin-top:10px"><span class="tag ${esc(x.status)}">${esc(x.status)}</span></div></div>`
    ).join('');
    el.querySelectorAll('.sys-card').forEach(c => c.onclick = async () => {
      try {
        const uid = c.dataset.u;
        await api('/api/admin/sys/act', { method: 'POST', body: JSON.stringify({ uid }) });
        await enter(uid);
      } catch (e) { alert('进入失败：' + (e.message || e)); }
    });
  } catch (e) {
    $('sysList').innerHTML = '<p class="muted">加载失败：' + esc(e.message || e) + '</p>';
  }
}

async function enter(uid) {
  acting = uid;
  st = await loadState();
  $('pickView').style.display = 'none';
  $('walletView').style.display = 'flex';
  renderNav();
  renderView();
}

async function loadState() {
  const j = await api('/api/admin/sys/state');
  if (j.logged_in === false) { location.reload(); return; }
  return j;
}

async function refreshState() {
  st = await loadState();
  renderTop();
  renderView();
}

function renderTop() {
  const who = $('who');
  if (st && who && !who.textContent.includes('→')) who.textContent += ' → ' + st.uid;
}

function renderNav() {
  const nav = $('wnav');
  nav.innerHTML = NAV.map(([k, l]) => `<div class="it ${view === k ? 'on' : ''}" data-v="${k}">${l}</div>`).join('') +
    '<div class="it" style="color:var(--red)" id="nav-quit">退出账本</div>';
  nav.querySelectorAll('.it[data-v]').forEach(el => el.onclick = () => { view = el.dataset.v; renderView(); });
  $('nav-quit').onclick = async () => {
    try { await api('/api/admin/sys/logout', { method: 'POST' }); } catch (e) {}
    location.reload();
  };
}

function renderView() {
  const map = { overview: vOverview, send: vSend, inbox: vInbox, txs: vTxs };
  const el = $('wmain');
  el.innerHTML = (map[view] || vOverview)();
  if (view === 'send') bindSend();
  if (view === 'inbox') bindInbox();
}

function dirSign(t) { return t.direction >= 0 ? '+' : '-'; }
function dirClass(t) { return t.direction >= 0 ? 'ok' : 'danger'; }

function vOverview() {
  if (!st) return '<p class="muted">加载中…</p>';
  const recent = (st.txs || []).slice(0, 6);
  return `<div class="hero"><div class="k">系统账本余额 · ${esc(st.uid)}</div><div class="v">${fmt(st.balance)} <span style="font-size:17px">A€</span></div>
    <div class="s">${esc(st.email)} · ${typeName('System')} · 最近刷新 ${tsFmt(st.synced_at)}</div></div>
    <div class="cards">
      <div class="card"><div class="k">账户类型</div><div class="v" style="font-size:18px">${typeName('System')}</div></div>
      <div class="card"><div class="k">待收箱</div><div class="v" style="font-size:18px">${(st.pending || []).length} 笔</div></div>
      <div class="card"><div class="k">最近刷新</div><div class="v" style="font-size:15px">${tsFmt(st.synced_at)}</div></div>
    </div>
    <div class="row"><button class="btn-primary btn-sm" onclick="refreshState()">⟳ 刷新</button></div>
    <div class="panel"><h2>最近流水</h2>${recent.length ? recent.map(txLine).join('') : '<p class="muted">暂无交易</p>'}</div>`;
}
function txLine(t) {
  return `<div class="list-row"><div class="g"><b>${esc(t.peer)}</b> <span class="muted">· ${esc(t.tx_type)} ${tsFmt(t.ts)}</span><br><span class="tag ${esc(t.status)}">${esc(t.status)}</span></div>
    <div class="${dirClass(t)}" style="font-weight:700">${dirSign(t)}${fmt(t.amount)} A€</div></div>`;
}
function vSend() {
  return `<div class="panel"><h2>转账（系统账本 → 目标账户）</h2>
    <div class="field"><label>接收方 UID</label><input id="s-to" placeholder="如 Steve 或 AlphaEU@System"></div>
    <div class="rowline"><label style="margin-right:8px">接收方类型</label>
      <select id="s-type"><option value="Individual">个人</option><option value="Company">企业（银行）</option><option value="Country">国家</option><option value="System">系统</option></select></div>
    <div class="field"><label>金额（A€）</label><input id="s-amt" type="number" min="1" placeholder="0"></div>
    <div class="row"><button class="btn-primary" id="s-go">确认转账</button><span class="muted" id="s-msg" style="margin-left:8px"></span></div></div>`;
}
async function bindSend() {
  $('s-go').onclick = async () => {
    const to = $('s-to').value.trim();
    const amt = parseInt($('s-amt').value || '0', 10);
    if (!to) { $('s-msg').textContent = '请输入接收方'; return; }
    if (!(amt > 0)) { $('s-msg').textContent = '金额须大于 0'; return; }
    $('s-msg').textContent = '提交中…';
    try {
      const r = await api('/api/admin/sys/transfer', {
        method: 'POST', body: JSON.stringify({ to, type: $('s-type').value, amount: amt })
      });
      $('s-msg').textContent = r.message || '已提交';
      await refreshState();
    } catch (e) { $('s-msg').textContent = (e.message || e); }
  };
}
function vInbox() {
  const list = (st && st.pending) || [];
  if (!list.length) return '<div class="panel"><h2>待收箱</h2><p class="muted">当前没有待确认交易。</p></div>';
  return `<div class="panel"><h2>待收箱</h2><table><thead><tr><th>发送方</th><th>类型</th><th>金额</th><th>时间</th><th style="width:150px">操作</th></tr></thead><tbody>` +
    list.map((p, i) => `<tr><td>${esc(p.sender)}</td><td>${esc(p.tx_type)}</td><td>${fmt(p.amount)} A€</td><td>${tsFmt(p.timestamp)}</td>
      <td><button class="btn-success btn-sm" data-i="${i}" data-a="ok">接受</button>
          <button class="btn-danger btn-sm" data-i="${i}" data-a="no">拒收</button></td></tr>`).join('') + '</tbody></table></div>';
}
async function bindInbox() {
  const list = (st && st.pending) || [];
  document.querySelectorAll('#wmain button[data-a]').forEach(b => b.onclick = async () => {
    const p = list[+b.dataset.i];
    if (!p) return;
    const act = b.dataset.a === 'ok' ? 'confirm' : 'reject';
    const reason = act === 'reject' ? (prompt('拒收原因（可选）') || '') : '';
    try {
      const r = await api('/api/admin/sys/' + act, {
        method: 'POST', body: JSON.stringify({ tx_id: p.tx_id, reason })
      });
      alert(r.message || '完成');
      await refreshState();
    } catch (e) { alert((e.message || e)); }
  });
}
function vTxs() {
  const txs = (st && st.txs) || [];
  if (!txs.length) return '<div class="panel"><h2>流水</h2><p class="muted">暂无交易。</p></div>';
  return `<div class="panel"><h2>流水 · 系统账本（${txs.length} 笔）</h2>${txs.map(txLine).join('')}</div>`;
}
function goBack() { location.href = '/root'; }
initSys();
