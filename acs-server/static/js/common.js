// 公共：API 封装、令牌、格式化、字符徽标、退出、提示、明暗主题、品牌化。
// 徽标：默认使用货币符号字符，配合 .logo 的渐变方形样式呈现（无 SVG/图片依赖）。
let LOGO_BADGE = 'A€';
/// 品牌配置：由 `/api/brand` 注入（服务端 `.env` 的 `ACS_BRAND_*`），未获取前用默认值。
let BRAND = { name: 'Alpha Coin', currency: 'A€', system_name: 'Alpha Coin System', union_abbr: 'AEU' };

let TOKEN = localStorage.getItem('acs_token') || '';

// ---------- 明暗主题切换 ----------
function applyTheme(){
  let t = localStorage.getItem('acs_theme') || 'light';
  document.documentElement.setAttribute('data-theme', t);
}
function toggleTheme(){
  const cur = document.documentElement.getAttribute('data-theme') === 'dark' ? 'light' : 'dark';
  document.documentElement.setAttribute('data-theme', cur);
  localStorage.setItem('acs_theme', cur);
}
applyTheme();

/// 统一 HTTP 错误文案：`HTTP <状态码> : <原因>`（与客户端一致，方便对照排查）
const httpMsg = (code, reason) => 'HTTP ' + code + ' : ' + ((reason && String(reason).trim()) || '请求被拒绝');

async function api(path, opt = {}) {
  const h = Object.assign({ 'Content-Type': 'application/json' }, opt.headers || {});
  if (TOKEN) h['Authorization'] = 'Bearer ' + TOKEN;
  const r = await fetch(path, Object.assign({}, opt, { headers: h }));
  if (opt.raw) {
    if (!r.ok) { const j = await r.json().catch(() => ({})); throw new Error(httpMsg(r.status, j.error)); }
    return r.text();
  }
  const j = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(httpMsg(r.status, j.error));
  return j;
}

const $ = id => document.getElementById(id);
const fmt = n => (n ?? 0).toLocaleString();
const fmtA = n => fmt(Math.round((n ?? 0) * 100) / 100);
const typeName = t => ({ System: '系统', Company: '企业', Country: '国家', Individual: '个人' }[t] || t);
const esc = s => String(s ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const tsFmt = s => s ? new Date(s * 1000).toLocaleString('zh-CN', { hour12: false, year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', second: '2-digit' }) : '-';

// ---------- 品牌化：ACS_BRAND_* → 界面文案 ----------
/// 替换品牌串（长串优先，避免部分替换）。
function brandSwap(text, b) {
  return String(text)
    .replace(/Alpha Coin System/g, b.system_name || 'Alpha Coin System')
    .replace(/Alpha Coin/g, b.name || 'Alpha Coin')
    .replace(/A€/g, b.currency || 'A€');
}
async function applyBrand() {
  try {
    const b = await api('/api/brand');
    if (!b || b.ok === false) return;
    BRAND = b;
    LOGO_BADGE = b.currency || LOGO_BADGE;
    document.title = brandSwap(document.title, b);
    // 只改「品牌容器」里的静态文本节点，不碰动态生成的业务内容
    document.querySelectorAll('.brand').forEach(el => {
      el.childNodes.forEach(n => { if (n.nodeType === 3) n.textContent = brandSwap(n.textContent, b); });
    });
    document.querySelectorAll('.logo, .brand-mark').forEach(el => {
      if (el.textContent.trim() === 'A€') el.textContent = b.currency || 'A€';
    });
  } catch (e) { /* 品牌获取失败不影响任何功能 */ }
}
window.addEventListener('load', () => applyBrand());

function msg(id, text) {
  const el = $(id);
  if (el) { el.textContent = text || ''; }
}

async function logout() {
  try { await api('/api/admin/logout', { method: 'POST' }); } catch (e) {}
  TOKEN = ''; localStorage.removeItem('acs_token');
  location.href = '/login';
}

// 折线图（纯 SVG，无第三方依赖，离线可用；颜色随主题自适应）
function drawLineChart(elId, items) {
  const el = $(elId); if (!el) return;
  if (!items || items.length === 0) { el.innerHTML = '<p class="muted" style="padding:40px;text-align:center">暂无数据</p>'; return; }
  const cs = getComputedStyle(document.documentElement);
  const textCol = cs.getPropertyValue('--text').trim() || '#1d1d1f';
  const mutCol = cs.getPropertyValue('--mut').trim() || '#6e6e73';
  const lineCol = cs.getPropertyValue('--line').trim() || '#eee';
  const W = 860, H = 240, padL = 56, padR = 16, padT = 16, padB = 32;
  const max = Math.max(...items.map(d => d.flow), 1);
  const innerW = W - padL - padR, innerH = H - padT - padB;
  const x = i => padL + (items.length === 1 ? innerW / 2 : (innerW * i) / (items.length - 1));
  const y = v => padT + innerH - (innerH * v) / max;
  let pts = items.map((d, i) => `${x(i).toFixed(1)},${y(d.flow).toFixed(1)}`).join(' ');
  let bars = items.map((d, i) =>
    `<rect x="${(x(i) - innerW / items.length / 3).toFixed(1)}" y="${y(d.flow).toFixed(1)}" width="${(innerW / items.length / 1.5).toFixed(1)}" height="${(innerH * d.flow / max).toFixed(1)}" fill="rgba(0,113,227,.18)" rx="3"/>`
  ).join('');
  let labels = items.map((d, i) =>
    `<text x="${x(i).toFixed(1)}" y="${H - 8}" text-anchor="middle" font-size="11" fill="${mutCol}">${esc(d.date.slice(5))}</text>`
  ).join('');
  let values = items.map((d, i) =>
    `<text x="${x(i).toFixed(1)}" y="${(y(d.flow) - 8).toFixed(1)}" text-anchor="middle" font-size="11" fill="${textCol}">${fmt(d.flow)}</text>`
  ).join('');
  // Y 轴刻度
  let yt = '';
  for (let g = 0; g <= 4; g++) {
    const v = Math.round((max * g) / 4);
    const yy = y(v);
    yt += `<line x1="${padL}" y1="${yy.toFixed(1)}" x2="${W - padR}" y2="${yy.toFixed(1)}" stroke="${lineCol}"/>` +
      `<text x="${padL - 8}" y="${(yy + 4).toFixed(1)}" text-anchor="end" font-size="11" fill="${mutCol}">${fmt(v)}</text>`;
  }
  el.innerHTML = `<svg class="chart-svg" viewBox="0 0 ${W} ${H}" preserveAspectRatio="xMidYMid meet">
    ${yt}${bars}<polyline points="${pts}" fill="none" stroke="#0071e3" stroke-width="2.5" stroke-linejoin="round" stroke-linecap="round"/>${labels}${values}
  </svg>`;
}
