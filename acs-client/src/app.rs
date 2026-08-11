//! TUI 应用状态机：首次引导（Onboarding） + 主界面（Overview/Tx/Accounts/Settings）。
//! 操作逻辑参考 OpenCode：顶部状态栏、分区导航、底部命令输入栏、常驻快捷键帮助。

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use acs_core::models::AccountType;

use crate::client_api;
use crate::sync::{self, SyncResult};
use crate::txn;
use crate::wallet::Wallet;

/// 应用模式。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Onboarding,
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
            View::Overview => "总览",
            View::Transactions => "交易",
            View::Accounts => "账户",
            View::Outbox => "待提交",
            View::Settings => "设置",
        }
    }
    pub fn idx(self) -> usize {
        View::ALL.iter().position(|v| *v == self).unwrap_or(0)
    }
}

/// 输入模式（底部输入栏）。
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

/// 待提交的转账意图（口令输入完成后执行）。
struct PendingSend {
    receiver: String,
    receiver_type: AccountType,
    amount: i64,
}

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
    pub running: bool,
}

impl App {
    pub fn new(wallet: Wallet) -> App {
        let initialized = wallet.info.initialized();
        App {
            mode: if initialized { Mode::Main } else { Mode::Onboarding },
            view: View::Overview,
            input_mode: InputMode::None,
            input: String::new(),
            pending_send: None,
            pending_confirm: None,
            status: if initialized { "钱包已就绪".into() } else { "首次使用，请完成引导".into() },
            help_visible: false,
            sync_res: None,
            txs: Vec::new(),
            outbox: Vec::new(),
            accounts: Vec::new(),
            onboard: Onboard::default(),
            running: true,
            wallet,
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
                let _ = self
                    .wallet
                    .mark_synced(r.server_time, None);
                self.sync_res = Some(r);
                self.status = format!(
                    "已同步：新增交易 {}，账户快照 {}",
                    self.sync_res.as_ref().map(|s| s.txs).unwrap_or(0),
                    self.sync_res.as_ref().map(|s| s.accounts).unwrap_or(0)
                );
                self.load_txs();
                self.load_accounts();
            }
            Err(e) => {
                self.status = format!("同步失败：{e}");
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
                    self.status = format!("已签名入 outbox：{tx_id}（hash {:.12}…）", tx_hash);
                    self.load_outbox();
                }
                Err(e) => {
                    self.status = format!("转账失败：{e}");
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
                    self.status = format!("交易 {:.8}… 状态 → {st}", tid);
                }
                Err(e) => {
                    self.status = format!("确认失败：{e}");
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
            Mode::Main => self.handle_main(key),
        }
    }

    // ---- 主界面 ----
    fn handle_main(&mut self, key: KeyEvent) {
        match self.input_mode {
            InputMode::None => self.main_nav(key),
            InputMode::Command => self.handle_input(key, false),
            InputMode::Passphrase => self.handle_input(key, true),
        }
    }

    fn main_nav(&mut self, key: KeyEvent) {
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
            KeyCode::Char(':') | KeyCode::Char('/') => {
                self.input.clear();
                self.input_mode = InputMode::Command;
            }
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
                match client_api::open_account(&self.wallet) {
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
                        self.status = format!("提交完成：{} 笔（{}）", results.len(), summary.join(", "));
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
                        self.status = "可先在【设置】中填写中心地址与镜像 apikey，再按 r 同步".into();
                    }
                    _ => {}
                }
            }
            OnboardStep::NetConfig => self.onboard_text(key, OnboardField::ServerUrl, OnboardField::ApiKey),
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
                            self.onboard.field = OnboardField::ApiKey;
                        }
                    } else {
                        self.onboard.step = OnboardStep::Identity;
                        self.onboard.field = OnboardField::Uid;
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
            Ok((_gk, uid, atype, email)) => {
                let _ = self.wallet.set_server_url(&self.onboard.server_url);
                let _ = self.wallet.set_mirror_apikey(&self.onboard.apikey);
                let _ = self.wallet.init_wallet(&uid, atype, &email);
                self.onboard.busy = false;
                self.onboard.step = OnboardStep::Done;
                self.status = "引导完成，欢迎使用 Alpha Wallet！按 Enter 进入主界面".into();
            }
            Err(e) => {
                self.onboard.busy = false;
                self.onboard.step = OnboardStep::ConfirmPass;
                self.onboard.error = Some(format!("密钥生成失败：{e}"));
            }
        }
    }
}
