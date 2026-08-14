// 公共：API 封装、令牌、格式化、字符徽标（A€）、退出、提示。
// 徽标：直接使用 A€ 字符，配合 .logo 的渐变方形样式呈现（无 SVG/图片依赖）。
const LOGO_BADGE = 'A€';

let TOKEN = localStorage.getItem('acs_token') || '';

async function api(path, opt = {}) {
  const h = Object.assign({ 'Content-Type': 'application/json' }, opt.headers || {});
  if (TOKEN) h['Authorization'] = 'Bearer ' + TOKEN;
  const r = await fetch(path, Object.assign({}, opt, { headers: h }));
  if (opt.raw) {
    if (!r.ok) { const j = await r.json().catch(() => ({})); throw new Error(j.error || ('HTTP ' + r.status)); }
    return r.text();
  }
  const j = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(j.error || ('HTTP ' + r.status));
  return j;
}

const $ = id => document.getElementById(id);
const fmt = n => (n ?? 0).toLocaleString();
const fmtA = n => fmt(Math.round((n ?? 0) * 100) / 100);
const typeName = t => ({ System: '系统', Company: '企业', Country: '国家', Individual: '个人' }[t] || t);
const esc = s => String(s ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const tsFmt = s => s ? new Date(s * 1000).toLocaleString('zh-CN', { hour12: false }) : '-';

function msg(id, text) {
  const el = $(id);
  if (el) { el.textContent = text || ''; }
}

async function logout() {
  try { await api('/api/admin/logout', { method: 'POST' }); } catch (e) {}
  TOKEN = ''; localStorage.removeItem('acs_token');
  location.href = '/login';
}

// 折线图（纯 SVG，无第三方依赖，离线可用）
function drawLineChart(elId, items) {
  const el = $(elId); if (!el) return;
  if (!items || items.length === 0) { el.innerHTML = '<p class="muted" style="padding:40px;text-align:center">暂无数据</p>'; return; }
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
    `<text x="${x(i).toFixed(1)}" y="${H - 8}" text-anchor="middle" font-size="11" fill="#6e6e73">${esc(d.date.slice(5))}</text>`
  ).join('');
  let values = items.map((d, i) =>
    `<text x="${x(i).toFixed(1)}" y="${(y(d.flow) - 8).toFixed(1)}" text-anchor="middle" font-size="11" fill="#1d1d1f">${fmt(d.flow)}</text>`
  ).join('');
  // Y 轴刻度
  let yt = '';
  for (let g = 0; g <= 4; g++) {
    const v = Math.round((max * g) / 4);
    const yy = y(v);
    yt += `<line x1="${padL}" y1="${yy.toFixed(1)}" x2="${W - padR}" y2="${yy.toFixed(1)}" stroke="#eee"/>` +
      `<text x="${padL - 8}" y="${(yy + 4).toFixed(1)}" text-anchor="end" font-size="11" fill="#6e6e73">${fmt(v)}</text>`;
  }
  el.innerHTML = `<svg class="chart-svg" viewBox="0 0 ${W} ${H}" preserveAspectRatio="xMidYMid meet">
    ${yt}${bars}<polyline points="${pts}" fill="none" stroke="#0071e3" stroke-width="2.5" stroke-linejoin="round" stroke-linecap="round"/>${labels}${values}
  </svg>`;
}
