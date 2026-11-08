// 登录页逻辑：登录 → 强制改密 → 按角色跳转。
document.querySelector('.logo').innerHTML = LOGO_BADGE;

let pendingToken = null;

async function doLogin() {
  const uid = $('uid').value.trim();
  const password = $('password').value;
  if (!uid || !password) { msg('loginMsg', '请输入账号与密码'); return; }
  try {
    const d = await api('/api/admin/login', { method: 'POST', body: JSON.stringify({ uid, password }) });
    TOKEN = d.token; localStorage.setItem('acs_token', TOKEN);
    if (d.admin.must_change_password) {
      pendingToken = TOKEN;
      $('loginForm').classList.add('hidden');
      $('chgForm').classList.remove('hidden');
      $('np').focus();
      return;
    }
    gotoHome(d.admin.role);
  } catch (e) { msg('loginMsg', e.message); }
}

async function doChange() {
  const np = $('np').value;
  const np2 = $('np2').value;
  if (np.length < 8) { msg('chgMsg', '新密码至少 8 位'); return; }
  if (!np2) { msg('chgMsg', '请再次输入新密码以确认'); return; }
  if (np !== np2) { msg('chgMsg', '两次输入的新密码不一致'); return; }
  try {
    // 首次登录强制改密：服务端在 must_change 状态下允许不传旧密码，但必须二次确认。
    const d = await api('/api/admin/change-password', {
      method: 'POST',
      body: JSON.stringify({ new_password: np, new_password_confirm: np2 }),
    });
    msg('chgMsg', '密码已修改，请使用新密码重新登录');
    localStorage.removeItem('acs_token');
    setTimeout(() => location.href = '/login', 800);
  } catch (e) { msg('chgMsg', e.message); }
}

function gotoHome(role) {
  location.href = (role === 'root') ? '/root' : '/admin';
}
