//! TUI 应用状态机：首次引导（Onboarding） + 主界面（Overview/Tx/Accounts/Settings）。
//! 操作逻辑参考 OpenCode：顶部状态栏、分区导航、底部命令输入栏、常驻快捷键帮助。

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Rect, Size};

use acs_core::models::AccountType;

use crate::client_api;
use crate::sync::{self, SyncResult};
use crate::txn;
use crate::wallet::Wallet;

/// 应用模式。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Onboarding,
    Login,
    Main,
}

/// 主界面视图。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum View {
    Overview,
    Transactions,
    Accounts,
    Outbox,
    Settings,
}

impl View {
    pub const ALL: [View; 5] = [
        View::Overview,
        View::Transactions,
        View::Accounts,
        View::Outbox,
        View::Settings,
    ];
    pub fn title(self) -> &'static str {
        match self {
            View::Overview => "首页",
            View::Transactions => "交易记录",
            View::Accounts => "我的账户",
            View::Outbox => "待提交",
            View::Settings => "设置",
        }
    }
    pub fn idx(self) -> usize {
        View::ALL.iter().position(|v| *v == self).unwrap_or(0)
    }
}

/// 输入模式（底部输入栏；1.3.0 起命令操作已弃用，保留兼容）。
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    None,
    Command,   // "> " 命令输入
    Passphrase, // 转账口令（隐藏）
}

/// 引导步骤。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OnboardStep {
    Welcome,
    NetConfig,    // 中心地址 + 镜像 apikey
    Identity,     // UID
    TypeSelect,   // 账户类型
    Email,        // 邮箱
    Passphrase,   // 口令
    ConfirmPass,  // 确认口令
    Generating,   // 生成密钥（busy）
    Done,         // 完成
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OnboardField {
    ServerUrl,
    ApiKey,
    Uid,
    Email,
    Pass1,
    Pass2,
}

pub struct Onboard {
    pub step: OnboardStep,
    pub field: OnboardField,
    pub server_url: String,
    pub apikey: String,
    pub uid: String,
    pub atype: AccountType,
    pub email: String,
    pub pass1: String,
    pub pass2: String,
    pub error: Option<String>,
    pub busy: bool,
}

impl Default for Onboard {
    fn default() -> Self {
        Onboard {
            step: OnboardStep::Welcome,
            field: OnboardField::ServerUrl,
            server_url: String::new(),
            apikey: String::new(),
            uid: String::new(),
            atype: AccountType::Individual,
            email: String::new(),
            pass1: String::new(),
            pass2: String::new(),
            error: None,
            busy: false,
        }
    }
}

/// 登录屏（多账户选择 + 密码）。
pub struct Login {
    /// (uid, type, 有本地密钥缓存, 最近登录时间戳)
    pub accounts: Vec<(String, String, bool, i64)>,
    pub sel: usize,
    pub password: String,
    pub show_pass: bool,
    pub error: Option<String>,
    pub busy: bool,
}

impl Default for Login {
    fn default() -> Self {
        Login {
            accounts: Vec::new(),
            sel: 0,
            password: String::new(),
            show_pass: false,
            error: None,
            busy: false,
        }
    }
}

/// 弹窗式状态提醒（自动换行，Esc/Enter 关闭或数秒后消失）。
pub struct Popup {
    pub title: String,
    pub msg: String,
    pub created: Instant,
}

/// 鼠标点击命中目标。
#[derive(Clone, Debug)]
pub enum HitTarget {
    Nav(usize),        // 左侧导航菜单第 i 项
    Quit,              // 左侧菜单退出项
    Button(String),    // 内容区操作按钮（action id）
    FormField(usize),  // 表单字段
    FormOk,            // 表单确定
    FormCancel,        // 表单取消
}

/// 可点击区域（与 UI 渲染布局保持一致）。
#[derive(Clone, Debug)]
pub struct HitArea {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
    pub target: HitTarget,
}

/// 表单类型（全鼠标菜单式操作，文本用键盘输入）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FormKind {
    Transfer,  // 转账：接收方 / 金额 / 口令
    Confirm,   // 确认收款：口令
    SetServer, // 设置中心地址
}

/// 表单弹窗状态。
pub struct Form {
    pub kind: FormKind,
    pub fields: Vec<String>,
    pub focus: usize,
    pub error: Option<String>,
}

/// 待提交的转账意图（口令输入完成后执行；1.3.0 起命令操作弃用，保留兼容）。
#[allow(dead_code)]
struct PendingSend {
    receiver: String,
    receiver_type: AccountType,
    amount: i64,
}

#[allow(dead_code)] // input_mode/input/pending_* 为旧命令栏遗留（1.3.0 起弃用，保留兼容）
pub struct App {
    pub wallet: Wallet,
    pub mode: Mode,
    pub view: View,
    pub input_mode: InputMode,
    pub input: String,
    pending_send: Option<PendingSend>,
    pending_confirm: Option<String>,
    pub status: String,
    /// 是否在内容区显示多行帮助面板。
    pub help_visible: bool,

    // 缓存数据
    pub sync_res: Option<SyncResult>,
    pub txs: Vec<(String, String, String, String, i64, i64, String)>,
    pub outbox: Vec<(String, String, i64)>,
    pub accounts: Vec<(String, String, i64, String)>,

    pub onboard: Onboard,
    pub login: Login,
    pub popup: Option<Popup>,
    pub hits: Vec<HitArea>,
    pub form: Option<Form>,
    pub running: bool,
}

#[allow(dead_code)] // 旧命令栏方法（begin_send 等）1.3.0 起弃用，保留兼容
impl App {
    pub fn new(wallet: Wallet) -> App {
        let initialized = wallet.info.initialized();
        let accounts = wallet
            .list_local_accounts()
            .into_iter()
            .map(|a| {
                (
                    a.uid,
                    a.atype.as_str().to_string(),
                    !a.encrypted_seckey.is_empty(),
                    a.last_login,
                )
            })
            .collect::<Vec<_>>();
        let mode = if initialized {
            Mode::Main
        } else if !accounts.is_empty() {
            Mode::Login
        } else {
            Mode::Onboarding
        };
        let status = if initialized {
            "钱包已就绪"
        } else if !accounts.is_empty() {
            "请选择账户登录"
        } else {
            "首次使用，请完成注册"
        };
        App {
            mode,
            view: View::Overview,
            input_mode: InputMode::None,
            input: String::new(),
            pending_send: None,
            pending_confirm: None,
            status: status.into(),
            help_visible: false,
            sync_res: None,
            txs: Vec::new(),
            outbox: Vec::new(),
            accounts: Vec::new(),
            onboard: Onboard::default(),
            login: Login {
                accounts,
                ..Default::default()
            },
            popup: None,
            hits: Vec::new(),
            form: None,
            running: true,
            wallet,
        }
    }

    /// 每次绘制前收集可点击区域（与 ui.rs 布局保持一致）。
    pub fn collect_hits(&mut self, size: Size) {
        self.hits.clear();
        if self.mode != Mode::Main {
            return;
        }
        let area = Rect {
            x: 0,
            y: 0,
            width: size.width,
            height: size.height,
        };
        let body_y = area.y + 3; // 顶部状态栏高 3
        // 左侧中文菜单（6 项：5 视图 + 退出）
        for i in 0..6 {
            let y = body_y + i as u16;
            if y < area.y + area.height {
                let target = if i == 5 { HitTarget::Quit } else { HitTarget::Nav(i) };
                self.hits.push(HitArea {
                    x: area.x,
                    y,
                    w: 14,
                    h: 1,
                    target,
                });
            }
        }
        // 内容区子菜单按钮行（nav 宽 14 + 内容左边框 1；每按钮 12 列）
        let content_x = area.x + 14 + 1;
        let btn_y = body_y + 1;
        for (i, a) in self.view_actions().iter().enumerate() {
            self.hits.push(HitArea {
                x: content_x + i as u16 * 12,
                y: btn_y,
                w: 12,
                h: 1,
                target: HitTarget::Button(a.to_string()),
            });
        }
        // 表单弹窗字段与按钮
        if let Some(f) = &self.form {
            let w = area.width.min(60).saturating_sub(4).max(34);
            let n = f.fields.len() as u16;
            let h = n.saturating_add(6).min(area.height.saturating_sub(2).max(10));
            let bx = area.x + area.width.saturating_sub(w) / 2;
            let by = area.y + area.height.saturating_sub(h) / 2;
            for i in 0..f.fields.len() {
                let y = by + 2 + i as u16;
                self.hits.push(HitArea {
                    x: bx + 2,
                    y,
                    w: w.saturating_sub(4),
                    h: 1,
                    target: HitTarget::FormField(i),
                });
            }
            let ok_y = by + h.saturating_sub(2);
            self.hits.push(HitArea {
                x: bx + 2,
                y: ok_y,
                w: 10,
                h: 1,
                target: HitTarget::FormOk,
            });
            self.hits.push(HitArea {
                x: bx + w.saturating_sub(12),
                y: ok_y,
                w: 10,
                h: 1,
                target: HitTarget::FormCancel,
            });
        }
    }

    /// 当前视图的子菜单操作按钮（action id，与 ui.rs 渲染一致）。
    fn view_actions(&self) -> Vec<&'static str> {
        match self.view {
            View::Overview => vec!["sync", "transfer", "refresh"],
            View::Accounts => vec!["sync", "switch", "register"],
            View::Transactions => vec!["transfer", "confirm", "submit"],
            View::Outbox => vec!["submit", "refresh"],
            View::Settings => vec!["setserver", "changepass", "relogin"],
        }
    }

    /// 处理鼠标左键点击（坐标命中测试）。
    pub fn handle_mouse(&mut self, x: u16, y: u16) {
        // 弹窗打开：任意点击关闭
        if self.popup.is_some() {
            self.popup = None;
            return;
        }
        // 先找出命中的目标（克隆出来，避免借用冲突）
        let hit_target: Option<HitTarget> = self
            .hits
            .iter()
            .find(|hit| x >= hit.x && x < hit.x + hit.w && y >= hit.y && y < hit.y + hit.h)
            .map(|hit| hit.target.clone());
        // 表单打开：仅响应表单字段/按钮；点击表单外关闭
        if self.form.is_some() {
            match &hit_target {
                Some(HitTarget::FormField(i)) => {
                    let i = *i;
                    self.form_set_focus(i);
                }
                Some(HitTarget::FormOk) => self.form_submit(),
                Some(HitTarget::FormCancel) => self.close_form(),
                _ => self.close_form(),
            }
            return;
        }
        match hit_target {
            Some(HitTarget::Nav(i)) => {
                self.view = View::ALL[i];
                self.refresh_view();
            }
            Some(HitTarget::Quit) => self.quit(),
            Some(HitTarget::Button(a)) => self.run_action(&a),
            _ => {}
        }
    }

    /// 执行内容区子菜单操作（全鼠标）。
    fn run_action(&mut self, a: &str) {
        match a {
            "sync" => self.do_sync(),
            "transfer" => self.open_form(FormKind::Transfer),
            "confirm" => self.open_form(FormKind::Confirm),
            "submit" => self.run_submit(),
            "refresh" => self.refresh_view(),
            "switch" | "relogin" => {
                self.mode = Mode::Login;
                self.login.password.clear();
                self.login.busy = false;
                self.login.error = None;
            }
            "register" => {
                self.mode = Mode::Onboarding;
                self.onboard.step = OnboardStep::Welcome;
            }
            "setserver" => self.open_form(FormKind::SetServer),
            "changepass" => {
                self.notify("更改口令", "口令在管理后台（9680）修改；客户端登录口令暂不支持在线修改")
            }
            _ => {}
        }
    }

    /// 打开表单（全鼠标；文本用键盘输入）。
    pub fn open_form(&mut self, kind: FormKind) {
        let fields = match kind {
            FormKind::Transfer => vec![String::new(), String::new(), String::new()],
            FormKind::Confirm => vec![String::new()],
            FormKind::SetServer => vec![self.wallet.info.server_url.clone()],
        };
        self.form = Some(Form {
            kind,
            fields,
            focus: 0,
            error: None,
        });
    }

    pub fn form_set_focus(&mut self, i: usize) {
        if let Some(f) = &mut self.form {
            if i < f.fields.len() {
                f.focus = i;
            }
        }
    }

    pub fn close_form(&mut self) {
        self.form = None;
    }

    fn form_error(&mut self, focus: usize, msg: &str) {
        if let Some(f) = &mut self.form {
            f.focus = focus;
            f.error = Some(msg.to_string());
        }
    }

    /// 提交表单（转账 / 确认 / 设置中心地址）。
    fn form_submit(&mut self) {
        let (kind, fields) = match &self.form {
            Some(f) => (f.kind, f.fields.clone()),
            None => return,
        };
        match kind {
            FormKind::Transfer => {
                let (to, amt, pass) = (fields[0].clone(), fields[1].clone(), fields[2].clone());
                let mut receiver = to.trim().to_string();
                let mut rtype = AccountType::Individual;
                if let Some(idx) = receiver.find('@') {
                    let ty = receiver[idx + 1..].to_string();
                    receiver = receiver[..idx].to_string();
                    rtype = AccountType::from_str(&ty).unwrap_or(AccountType::Individual);
                }
                if receiver.is_empty() {
                    return self.form_error(0, "请输入接收方 UID");
                }
                let amount: i64 = match amt.trim().parse() {
                    Ok(v) if v > 0 => v,
                    _ => return self.form_error(1, "金额须为大于 0 的数字"),
                };
                if pass.len() < 8 {
                    return self.form_error(2, "口令至少 8 位");
                }
                match txn::build_and_sign_transfer(&self.wallet, &receiver, rtype, amount, &pass) {
                    Ok((tx_id, tx_hash)) => {
                        self.form = None;
                        self.notify(
                            "转账成功",
                            &format!("已签名入待提交：{tx_id}\nhash {tx_hash:.12}…"),
                        );
                        self.load_outbox();
                    }
                    Err(e) => self.form_error(2, &e.to_string()),
                }
            }
            FormKind::Confirm => {
                let pass = fields[0].clone();
                if pass.len() < 8 {
                    return self.form_error(0, "口令至少 8 位");
                }
                // 取第一笔待确认交易
                let tid = client_api::list_pending(&self.wallet)
                    .ok()
                    .and_then(|list| list.into_iter().next().map(|p| p.tx_id));
                let tid = match tid {
                    Some(t) => t,
                    None => {
                        self.form = None;
                        self.notify("确认收款", "当前没有待确认交易");
                        return;
                    }
                };
                match client_api::confirm_tx(&self.wallet, &tid, &pass, None) {
                    Ok(r) => {
                        let st = r.get("status").and_then(|v| v.as_str()).unwrap_or("");
                        self.form = None;
                        self.notify("确认结果", &format!("交易 {:.8}… 状态 → {st}", tid));
                    }
                    Err(e) => self.form_error(0, &e.to_string()),
                }
            }
            FormKind::SetServer => {
                let raw = fields[0].trim().to_string();
                if raw.is_empty() {
                    return self.form_error(0, "请输入中心地址");
                }
                let u = if raw.starts_with("http://") || raw.starts_with("https://") {
                    raw
                } else {
                    format!("http://{raw}")
                };
                let _ = self.wallet.set_server_url(&u);
                self.form = None;
                self.notify("设置已保存", &format!("中心地址：{u}"));
            }
        }
    }

    /// 表单打开时键盘输入。
    fn handle_form_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.form = None,
            KeyCode::Tab => {
                if let Some(f) = &mut self.form {
                    f.focus = (f.focus + 1) % f.fields.len();
                }
            }
            KeyCode::Backspace => {
                if let Some(f) = &mut self.form {
                    f.fields[f.focus].pop();
                    f.error = None;
                }
            }
            KeyCode::Char(c) => {
                if let Some(f) = &mut self.form {
                    f.fields[f.focus].push(c);
                    f.error = None;
                }
            }
            KeyCode::Enter => self.form_submit(),
            _ => {}
        }
    }

    /// 提交全部待提交交易（鼠标菜单按钮）。
    pub fn run_submit(&mut self) {
        match client_api::submit_outbox(&self.wallet, None) {
            Ok(results) if results.is_empty() => {
                self.notify("提交", "outbox 中没有待提交交易");
            }
            Ok(results) => {
                let summary: Vec<String> = results.iter().map(|(_, s)| s.clone()).collect();
                self.notify(
                    "提交完成",
                    &format!("共提交 {} 笔\n{}", results.len(), summary.join("\n")),
                );
                self.load_outbox();
            }
            Err(e) => self.notify("提交失败", &e.to_string()),
        }
    }

    /// 转账按钮：打开转账表单。
    pub fn begin_send_input(&mut self) {
        self.open_form(FormKind::Transfer);
    }

    /// 弹窗式状态提醒（自动换行；Esc 关闭或 5 秒后消失）。
    pub fn notify(&mut self, title: &str, msg: &str) {
        self.popup = Some(Popup {
            title: title.to_string(),
            msg: msg.to_string(),
            created: Instant::now(),
        });
    }

    /// 每帧调用：弹窗超时自动关闭。
    pub fn tick(&mut self) {
        if let Some(p) = &self.popup {
            if p.created.elapsed() > Duration::from_secs(5) {
                self.popup = None;
            }
        }
    }

    pub fn refresh_view(&mut self) {
        match self.view {
            View::Overview | View::Transactions => self.load_txs(),
            View::Accounts => self.load_accounts(),
            View::Outbox => self.load_outbox(),
            View::Settings => {}
        }
    }

    pub fn load_txs(&mut self) {
        self.txs = txn::list_local_tx(&self.wallet, 200);
    }
    pub fn load_accounts(&mut self) {
        let mut stmt = self
            .wallet
            .conn
            .prepare(
                "SELECT uid, type, balance, status FROM mirror_accounts ORDER BY synced_at DESC, uid",
            )
            .unwrap();
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .unwrap();
        self.accounts = rows.flatten().collect();
    }
    pub fn load_outbox(&mut self) {
        self.outbox = txn::list_outbox(&self.wallet);
    }

    /// 执行同步（含引导/主界面共用）。
    pub fn do_sync(&mut self) {
        match sync::pull(&self.wallet) {
            Ok(r) => {
                let src = r.source.clone();
                let _ = self.wallet.mark_synced(r.server_time, None);
                self.sync_res = Some(r);
                let n_tx = self.sync_res.as_ref().map(|s| s.txs).unwrap_or(0);
                let n_acc = self.sync_res.as_ref().map(|s| s.accounts).unwrap_or(0);
                self.status = format!("已同步（{src}）");
                self.notify(
                    "同步完成",
                    &format!("已同步（{src}）\n新增交易 {n_tx}，账户快照 {n_acc}"),
                );
                self.load_txs();
                self.load_accounts();
            }
            Err(e) => {
                self.status = format!("同步失败：{e}");
                self.notify("同步失败", &e.to_string());
            }
        }
    }

    /// 启动转账（进入口令输入）。
    pub fn begin_send(&mut self, receiver: &str, atype: AccountType, amount: i64) {
        if amount <= 0 {
            self.status = "金额须大于 0".into();
            return;
        }
        self.pending_send = Some(PendingSend {
            receiver: receiver.trim().to_string(),
            receiver_type: atype,
            amount,
        });
        self.input.clear();
        self.input_mode = InputMode::Passphrase;
    }

    /// 口令输入完成后执行签名转账或确认。
    fn finish_password_action(&mut self) {
        if let Some(ps) = self.pending_send.take() {
            let pass = self.input.clone();
            self.input.clear();
            self.input_mode = InputMode::None;
            match txn::build_and_sign_transfer(
                &self.wallet,
                &ps.receiver,
                ps.receiver_type,
                ps.amount,
                &pass,
            ) {
                Ok((tx_id, tx_hash)) => {
                    self.notify(
                        "转账成功",
                        &format!("已签名入 outbox：{tx_id}\nhash {tx_hash:.12}…"),
                    );
                    self.load_outbox();
                }
                Err(e) => {
                    self.notify("转账失败", &e.to_string());
                }
            }
            return;
        }
        if let Some(tid) = self.pending_confirm.take() {
            let pass = self.input.clone();
            self.input.clear();
            self.input_mode = InputMode::None;
            match client_api::confirm_tx(&self.wallet, &tid, &pass, None) {
                Ok(r) => {
                    let st = r.get("status").and_then(|v| v.as_str()).unwrap_or("");
                    self.notify("确认结果", &format!("交易 {:.8}… 状态 → {st}", tid));
                }
                Err(e) => {
                    self.notify("确认失败", &e.to_string());
                }
            }
            return;
        }
        self.input.clear();
        self.input_mode = InputMode::None;
    }

    pub fn quit(&mut self) {
        self.running = false;
    }

    /// 处理键盘事件。返回是否退出。
    pub fn handle_key(&mut self, key: KeyEvent) {
        // Windows 下 crossterm 0.28 对每个按键会额外产生一次 Release 事件，
        // 若不过滤会导致“按一下跳两格”。只响应按下（Press）与长按（Repeat）。
        if key.kind == KeyEventKind::Release {
            return;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit();
            return;
        }
        match self.mode {
            Mode::Onboarding => self.handle_onboard(key),
            Mode::Login => self.handle_login(key),
            Mode::Main => self.handle_main(key),
        }
    }

    // ---- 主界面 ----
    fn handle_main(&mut self, key: KeyEvent) {
        // 弹窗打开时：Esc/Enter/空格 关闭，q 退出，其余忽略
        if self.popup.is_some() {
            match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char(' ') => self.popup = None,
                KeyCode::Char('q') => self.quit(),
                _ => {}
            }
            return;
        }
        // 表单打开：键盘输入到表单
        if self.form.is_some() {
            self.handle_form_key(key);
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.quit(),
            _ => self.main_nav(key),
        }
    }

    fn main_nav(&mut self, key: KeyEvent) {
        // 帮助面板打开时：仅响应 h/Esc 关闭、q 退出，忽略导航键（避免画面不变但高亮乱跳）
        if self.help_visible {
            match key.code {
                KeyCode::Char('h') | KeyCode::Char('H') => self.help_visible = false,
                KeyCode::Esc => self.help_visible = false,
                KeyCode::Char('q') => self.quit(),
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.quit(),
            // 上下键切换主视图（同级菜单）；左右键预留为上下级菜单导航（当前无子菜单，不切换视图）
            KeyCode::Down | KeyCode::Char('j') => {
                let i = (self.view.idx() + 1) % View::ALL.len();
                self.view = View::ALL[i];
                self.refresh_view();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let n = View::ALL.len();
                let i = (self.view.idx() + n - 1) % n;
                self.view = View::ALL[i];
                self.refresh_view();
            }
            KeyCode::Char('1') => self.set_view(View::Overview),
            KeyCode::Char('2') => self.set_view(View::Transactions),
            KeyCode::Char('3') => self.set_view(View::Accounts),
            KeyCode::Char('4') => self.set_view(View::Outbox),
            KeyCode::Char('5') => self.set_view(View::Settings),
            KeyCode::Char('r') | KeyCode::Char('R') => self.do_sync(),
            KeyCode::Char('h') | KeyCode::Char('H') => self.help_visible = !self.help_visible,
            KeyCode::Esc => self.help_visible = false,
            KeyCode::Enter => self.do_sync(),
            _ => {}
        }
    }

    fn set_view(&mut self, v: View) {
        self.view = v;
        self.refresh_view();
    }

    fn handle_input(&mut self, key: KeyEvent, hidden: bool) {
        match key.code {
            KeyCode::Enter => {
                if hidden {
                    self.finish_password_action();
                } else {
                    self.run_command();
                }
            }
            KeyCode::Esc => {
                self.input.clear();
                self.pending_send = None;
                self.pending_confirm = None;
                self.input_mode = InputMode::None;
            }
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Char(c) => {
                if !hidden || !c.is_whitespace() {
                    self.input.push(c);
                }
            }
            _ => {}
        }
    }

    /// 命令解析：send <uid> <amount> | sync | help | quit | balance
    fn run_command(&mut self) {
        let cmd = self.input.trim().to_string();
        self.input.clear();
        self.input_mode = InputMode::None;
        let parts: Vec<&str> = cmd.split_whitespace().collect();
        match parts.first().map(|s| s.to_lowercase()).as_deref() {
            Some("sync") | Some("s") => self.do_sync(),
            Some("send") | Some("t") => {
                if parts.len() < 3 {
                    self.status = "用法：send <接收方UID> <金额>".into();
                    return;
                }
                let amount: i64 = match parts[2].parse() {
                    Ok(a) => a,
                    Err(_) => {
                        self.status = "金额无效".into();
                        return;
                    }
                };
                // 接收方类型：默认 Individual；可加类型后缀如 name@Bank
                let mut receiver = parts[1].to_string();
                let mut rtype = AccountType::Individual;
                if let Some(idx) = receiver.find('@') {
                    let ty = receiver[idx + 1..].to_string();
                    receiver = receiver[..idx].to_string();
                    rtype = AccountType::from_str(&ty).unwrap_or(AccountType::Individual);
                }
                self.begin_send(&receiver, rtype, amount);
            }
            Some("balance") | Some("b") => {
                self.status = format!("本账户余额（镜像口径）：{} A€", txn::balance(&self.wallet));
            }
            Some("open") => {
                let sek = self.wallet.encrypted_seckey().unwrap_or_default();
                match client_api::open_account(&self.wallet, &sek, "") {
                    Ok(r) => {
                        let uid = r.get("uid").and_then(|v| v.as_str()).unwrap_or("");
                        let bal = r.get("balance").and_then(|v| v.as_i64()).unwrap_or(0);
                        self.status = format!("账户已开立：{uid}（余额 {bal} A€）");
                    }
                    Err(e) => self.status = format!("开立失败：{e}"),
                }
            }
            Some("submit") => {
                let tid = parts.get(1).map(|s| s.to_string());
                match client_api::submit_outbox(&self.wallet, tid.as_deref()) {
                    Ok(results) if results.is_empty() => {
                        self.status = "outbox 中没有待提交交易".into();
                    }
                    Ok(results) => {
                        let summary: Vec<String> = results.iter().map(|(_, s)| s.clone()).collect();
                        self.notify(
                            "提交完成",
                            &format!("共提交 {} 笔\n{}", results.len(), summary.join("\n")),
                        );
                        self.load_outbox();
                    }
                    Err(e) => self.status = format!("提交失败：{e}"),
                }
            }
            Some("confirm") => {
                let tid = parts.get(1).map(|s| s.to_string());
                match tid {
                    Some(id) => {
                        self.pending_confirm = Some(id);
                        self.input.clear();
                        self.input_mode = InputMode::Passphrase;
                    }
                    None => match client_api::list_pending(&self.wallet) {
                        Ok(list) if list.is_empty() => self.status = "没有待确认交易".into(),
                        Ok(list) => {
                            let p = &list[0];
                            self.pending_confirm = Some(p.tx_id.clone());
                            self.status = format!(
                                "有 {} 笔待确认，将确认 {:.8}…（{sender} → 我，{amt} A€），输入口令签名",
                                list.len(), p.tx_id, sender = p.sender, amt = p.amount
                            );
                            self.input.clear();
                            self.input_mode = InputMode::Passphrase;
                        }
                        Err(e) => self.status = format!("查询待确认失败：{e}"),
                    },
                }
            }
            Some("set") => {
                if parts.len() >= 3 && parts[1].eq_ignore_ascii_case("server") {
                    let mut u = parts[2..].join(" ");
                    if !u.starts_with("http://") && !u.starts_with("https://") {
                        u = format!("http://{u}");
                    }
                    match self.wallet.set_server_url(&u) {
                        Ok(_) => self.status = format!("中心地址已设置：{u}"),
                        Err(e) => self.status = format!("设置失败：{e}"),
                    }
                } else if parts.len() >= 3 && parts[1].eq_ignore_ascii_case("apikey") {
                    let k = parts[2..].join(" ");
                    match self.wallet.set_mirror_apikey(&k) {
                        Ok(_) => self.status = "镜像 apikey 已设置".into(),
                        Err(e) => self.status = format!("设置失败：{e}"),
                    }
                } else {
                    self.status = "用法：set server <地址> · set apikey <key>".into();
                }
            }
            Some("help") | Some("?") => {
                self.help_visible = true;
                self.status = "按 h 或 Esc 关闭帮助".into();
            }
            Some("quit") | Some("q") => self.quit(),
            _ => {
                self.status = "未知命令，输入 help 查看帮助".into();
            }
        }
    }

    // ---- 引导 ----
    fn handle_onboard(&mut self, key: KeyEvent) {
        if self.onboard.busy {
            return;
        }
        match self.onboard.step {
            OnboardStep::Welcome => {
                match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                        self.onboard.step = OnboardStep::NetConfig;
                        self.onboard.field = OnboardField::ServerUrl;
                    }
                    KeyCode::Char('n') | KeyCode::Char('N') => {
                        // 不引导：直接进设置视图手动配置
                        self.mode = Mode::Main;
                        self.view = View::Settings;
                        self.status = "可先在【设置】中填写中心地址，再按 r 同步".into();
                    }
                    _ => {}
                }
            }
            OnboardStep::NetConfig => self.onboard_text(key, OnboardField::ServerUrl, OnboardField::ServerUrl),
            OnboardStep::Identity => self.onboard_text(key, OnboardField::Uid, OnboardField::Uid),
            OnboardStep::Email => self.onboard_text(key, OnboardField::Email, OnboardField::Email),
            OnboardStep::Passphrase => self.onboard_text(key, OnboardField::Pass1, OnboardField::Pass1),
            OnboardStep::ConfirmPass => self.onboard_text(key, OnboardField::Pass2, OnboardField::Pass2),
            OnboardStep::TypeSelect => {
                let types = [AccountType::Individual, AccountType::Bank, AccountType::Country];
                match key.code {
                    KeyCode::Down | KeyCode::Char('j') => {
                        let i = types.iter().position(|t| *t == self.onboard.atype).unwrap_or(0);
                        self.onboard.atype = types[(i + 1) % types.len()];
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        let n = types.len();
                        let i = types.iter().position(|t| *t == self.onboard.atype).unwrap_or(0);
                        self.onboard.atype = types[(i + n - 1) % n];
                    }
                    KeyCode::Enter => {
                        if self.onboard.atype == AccountType::Country {
                            // 国家账户需先被 AEU 理事会登记，此处仍允许选择但提示
                            self.status = "国家账户需由 AEU 理事会登记。".into();
                        }
                        self.onboard.step = OnboardStep::Email;
                        self.onboard.field = OnboardField::Email;
                    }
                    _ => {}
                }
            }
            OnboardStep::Generating | OnboardStep::Done => {
                if key.code == KeyCode::Enter || key.code == KeyCode::Char('q') {
                    self.mode = Mode::Main;
                    self.view = View::Overview;
                    self.refresh_view();
                }
            }
        }
    }

    fn onboard_text(&mut self, key: KeyEvent, _f: OnboardField, next_f: OnboardField) {
        match key.code {
            KeyCode::Char(c) => self.onboard_push(c),
            KeyCode::Backspace => self.onboard_pop(),
            KeyCode::Enter => {
                if self.onboard.step == OnboardStep::NetConfig {
                    if self.onboard.field == OnboardField::ServerUrl {
                        let raw = self.onboard.server_url.trim().to_string();
                        if raw.is_empty() {
                            self.onboard.error = Some("请输入中心服务器地址（必填，如 http://localhost:8080）".into());
                        } else {
                            // 自动补全协议前缀
                            let u = if raw.starts_with("http://") || raw.starts_with("https://") {
                                raw
                            } else {
                                format!("http://{raw}")
                            };
                            self.onboard.server_url = u;
                            self.onboard.error = None;
                            self.onboard.step = OnboardStep::Identity;
                            self.onboard.field = OnboardField::Uid;
                        }
                    }
                } else {
                    // 校验并进入下一步
                    match self.onboard.step {
                        OnboardStep::Identity => {
                            if self.onboard.uid.trim().is_empty() {
                                self.onboard.error = Some("UID 不能为空".into());
                            } else {
                                self.onboard.error = None;
                                self.onboard.step = OnboardStep::TypeSelect;
                            }
                        }
                        OnboardStep::Email => {
                            if !self.onboard.email.contains('@') {
                                self.onboard.error = Some("请输入有效邮箱（如 user@aeu.org）".into());
                            } else {
                                self.onboard.error = None;
                                self.onboard.step = OnboardStep::Passphrase;
                                self.onboard.field = OnboardField::Pass1;
                            }
                        }
                        OnboardStep::Passphrase => {
                            if self.onboard.pass1.len() < 8 {
                                self.onboard.error = Some("口令至少 8 位".into());
                            } else {
                                self.onboard.error = None;
                                self.onboard.step = OnboardStep::ConfirmPass;
                                self.onboard.field = OnboardField::Pass2;
                            }
                        }
                        OnboardStep::ConfirmPass => {
                            if self.onboard.pass2 != self.onboard.pass1 {
                                self.onboard.error = Some("两次输入不一致".into());
                            } else {
                                self.onboard.error = None;
                                self.finish_onboard();
                            }
                        }
                        _ => {}
                    }
                }
            }
            KeyCode::Tab | KeyCode::Down => {
                self.onboard.field = next_f;
            }
            _ => {}
        }
    }

    fn onboard_push(&mut self, c: char) {
        self.onboard.error = None;
        let field = self.onboard.field;
        let s = match field {
            OnboardField::ServerUrl => &mut self.onboard.server_url,
            OnboardField::ApiKey => &mut self.onboard.apikey,
            OnboardField::Uid => &mut self.onboard.uid,
            OnboardField::Email => &mut self.onboard.email,
            OnboardField::Pass1 => &mut self.onboard.pass1,
            OnboardField::Pass2 => &mut self.onboard.pass2,
        };
        s.push(c);
    }

    fn onboard_pop(&mut self) {
        let field = self.onboard.field;
        let s = match field {
            OnboardField::ServerUrl => &mut self.onboard.server_url,
            OnboardField::ApiKey => &mut self.onboard.apikey,
            OnboardField::Uid => &mut self.onboard.uid,
            OnboardField::Email => &mut self.onboard.email,
            OnboardField::Pass1 => &mut self.onboard.pass1,
            OnboardField::Pass2 => &mut self.onboard.pass2,
        };
        s.pop();
    }

    fn finish_onboard(&mut self) {
        self.onboard.step = OnboardStep::Generating;
        self.onboard.busy = true;
        let uid = self.onboard.uid.trim().to_string();
        let atype = self.onboard.atype;
        let email = self.onboard.email.trim().to_string();
        let pass = self.onboard.pass1.clone();

        // 先生成密钥（可能耗时），再落库
        let result = self
            .wallet
            .create_key(&uid, &email, &pass)
            .map(|gk| (gk, uid, atype, email));
        match result {
            Ok((gk, uid, atype, email)) => {
                let _ = self.wallet.set_server_url(&self.onboard.server_url);
                let _ = self.wallet.init_wallet(&uid, atype, &email);
                // 密码哈希（$salt$sha256，与中心一致），供登录取回校验
                let salt = uuid::Uuid::new_v4().to_string();
                let password_hash =
                    format!("{salt}${}", sha256_hex(format!("{salt}:{pass}").as_bytes()));
                // 缓存加密私钥到本地账户清单（多账户互不干扰）
                let _ = self
                    .wallet
                    .save_local_account(&uid, atype, &email, &gk.encrypted_seckey);
                // 上传加密私钥 + 密码哈希到中心（注册即支持跨设备登录）
                let open_res =
                    client_api::open_account(&self.wallet, &gk.encrypted_seckey, &password_hash);
                self.onboard.busy = false;
                self.onboard.step = OnboardStep::Done;
                match open_res {
                    Ok(_) => {
                        self.status = "引导完成，账户已在中心登记，按 Enter 进入主界面".into();
                    }
                    Err(e) => {
                        self.status =
                            format!("引导完成，但中心登记失败：{e}（可稍后在设置重试）");
                    }
                }
            }
            Err(e) => {
                self.onboard.busy = false;
                self.onboard.step = OnboardStep::ConfirmPass;
                self.onboard.error = Some(format!("密钥生成失败：{e}"));
            }
        }
    }

    // ---- 登录（多账户） ----
    fn handle_login(&mut self, key: KeyEvent) {
        if self.login.busy {
            return;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit(),
            KeyCode::Up | KeyCode::Char('k') => {
                let n = self.login.accounts.len() + 1;
                self.login.sel = (self.login.sel + n - 1) % n;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let n = self.login.accounts.len() + 1;
                self.login.sel = (self.login.sel + 1) % n;
            }
            KeyCode::Tab => self.login.show_pass = !self.login.show_pass,
            KeyCode::Backspace => {
                self.login.password.pop();
                self.login.error = None;
            }
            KeyCode::Char(c) => {
                self.login.password.push(c);
                self.login.error = None;
            }
            KeyCode::Enter => {
                if self.login.accounts.is_empty() || self.login.sel >= self.login.accounts.len() {
                    // 注册新账户 → 引导
                    self.mode = Mode::Onboarding;
                    self.onboard.step = OnboardStep::Welcome;
                } else {
                    self.do_login(self.login.sel);
                }
            }
            _ => {}
        }
    }

    fn do_login(&mut self, idx: usize) {
        if idx >= self.login.accounts.len() {
            return;
        }
        let (uid, _t, _has_cache, _last) = self.login.accounts[idx].clone();
        let pass = self.login.password.clone();
        if pass.len() < 8 {
            self.login.error = Some("密码至少 8 位".into());
            return;
        }
        self.login.busy = true;
        self.login.error = None;

        // 1) 本地有加密私钥缓存 → 直接导入并用口令校验
        if let Some(acc) = self.wallet.local_account(&uid) {
            if !acc.encrypted_seckey.is_empty()
                && self.wallet.gpg.import_key(&acc.encrypted_seckey).is_ok()
                && self
                    .wallet
                    .fingerprint(&uid)
                    .and_then(|fp| self.wallet.gpg.verify_passphrase(&fp, &pass).ok())
                    .is_some()
            {
                self.login_done(&uid);
                return;
            }
        }
        // 2) 本地无缓存或口令不符 → 向中心取回（服务端校验密码哈希）
        self.fetch_and_login(&uid, &pass);
    }

    fn fetch_and_login(&mut self, uid: &str, pass: &str) {
        let atype = self
            .wallet
            .local_account(uid)
            .map(|a| a.atype)
            .unwrap_or(AccountType::Individual);
        match client_api::fetch_key(&self.wallet, uid, atype, pass) {
            Ok(r) => {
                let email = r
                    .get("email")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let sek = r
                    .get("encrypted_seckey")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if sek.is_empty() {
                    self.login.busy = false;
                    self.login.error =
                        Some("中心未存有该账户加密私钥（该账户非本客户端注册）".into());
                    return;
                }
                if let Err(e) = self.wallet.gpg.import_key(&sek) {
                    self.login.busy = false;
                    self.login.error = Some(format!("导入密钥失败：{e}"));
                    return;
                }
                let _ = self.wallet.save_local_account(uid, atype, &email, &sek);
                self.login_done(uid);
            }
            Err(e) => {
                self.login.busy = false;
                self.login.error = Some(format!("登录失败：{e}"));
            }
        }
    }

    fn login_done(&mut self, uid: &str) {
        let _ = self.wallet.switch_account(uid);
        self.login.busy = false;
        self.login.password.clear();
        self.mode = Mode::Main;
        self.view = View::Overview;
        self.refresh_view();
        self.status = format!("欢迎回来，{uid}");
        self.notify("登录成功", &format!("欢迎回来，{uid}"));
        // 自动从中心拉取镜像清单与账户快照（免手动输入 apikey）
        self.do_sync();
    }
}

/// sha256 十六进制（密码哈希，与中心一致）。
fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(data);
    let d = h.finalize();
    let mut s = String::with_capacity(d.len() * 2);
    for b in d {
        s.push_str(&format!("{b:02x}"));
    }
    s
}
