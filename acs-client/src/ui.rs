//! ratatui 渲染：OpenCode 风格深色终端界面。
//! 布局：顶部状态栏 / 中部（左侧导航 + 内容区）/ 底部（命令输入栏 + 帮助栏）。

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

use crate::app::{App, InputMode, Mode, OnboardStep, View};

// ---- OpenCode 风格配色 ----
const BG: Color = Color::Rgb(13, 17, 23);
const PANEL: Color = Color::Rgb(22, 27, 34);
const BORDER: Color = Color::Rgb(48, 54, 61);
const FG: Color = Color::Rgb(230, 237, 243);
const MUT: Color = Color::Rgb(139, 148, 158);
const ACCENT: Color = Color::Rgb(88, 166, 255); // 蓝
const ACCENT2: Color = Color::Rgb(188, 140, 255); // 紫
const OK: Color = Color::Rgb(63, 185, 80);
const WARN: Color = Color::Rgb(210, 153, 34);
const ERR: Color = Color::Rgb(248, 81, 73);

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    frame.render_widget(Block::default().style(Style::default().bg(BG)), area);
    // 极小终端防护：避免布局越界
    if area.width < 30 || area.height < 10 {
        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(
                "终端窗口太小，请放大后重试",
                Style::default().fg(MUT),
            )])),
            area,
        );
        return;
    }
    match app.mode {
        Mode::Onboarding => draw_onboard(frame, app),
        Mode::Main => draw_main(frame, app),
    }
}

// ---------------- 引导 ----------------
fn draw_onboard(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let w = area.width.min(72).saturating_sub(2).max(30);
    let h = area.height.min(26).saturating_sub(2).max(10);
    let box_area = Rect {
        x: area.x + area.width.saturating_sub(w) / 2,
        y: area.y + area.height.saturating_sub(h) / 2,
        width: w,
        height: h,
    };
    frame.render_widget(Clear, box_area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .title(Line::from(vec![
            Span::styled(" ● ", Style::default().fg(ACCENT2)),
            Span::styled("Alpha Wallet · 首次引导", Style::default().fg(FG).add_modifier(Modifier::BOLD)),
        ]));
    let inner = block.inner(box_area);
    frame.render_widget(block, box_area);

    let mut lines: Vec<Line> = Vec::new();
    match app.onboard.step {
        OnboardStep::Welcome => {
            lines.push(line_center(Span::styled("欢迎使用 A€（Alpha Coin）钱包", Style::default().fg(FG).add_modifier(Modifier::BOLD)), inner.width));
            lines.push(Line::default());
            lines.push(wrapped("Alpha Coin 是 AEU 的中央银行数字货币，由 Alpha Coin System 统一记账结算。", MUT, inner.width));
            lines.push(Line::default());
            lines.push(line_center(Span::styled("这是您首次启动，是否需要引导设置？", Style::default().fg(FG)), inner.width));
            lines.push(Line::default());
            lines.push(line_center(Span::styled("[Y] 是，开始引导      [N] 否，稍后手动设置", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)), inner.width));
        }
        OnboardStep::NetConfig => {
            lines.push(section("网络连接", inner.width));
            lines.push(field_label("中心服务器地址（必填）", "如 http://localhost:8080", app.onboard.field == crate::app::OnboardField::ServerUrl));
            lines.push(value_line(&app.onboard.server_url, app.onboard.field == crate::app::OnboardField::ServerUrl));
            lines.push(Line::default());
            lines.push(field_label("镜像 apikey（只读同步，向管理员索取，可留空）", "如 mir-xxxxxxxx", app.onboard.field == crate::app::OnboardField::ApiKey));
            lines.push(value_line(&app.onboard.apikey, app.onboard.field == crate::app::OnboardField::ApiKey));
            lines.push(Line::default());
            lines.push(hint("Tab 切换字段 · Enter 下一步 · Esc 上一步", inner.width));
        }
        OnboardStep::Identity => {
            lines.push(section("创建钱包身份", inner.width));
            lines.push(field_label("UID（你的游戏 ID / 用户名，唯一识别符）", "如 Steve", true));
            lines.push(value_line(&app.onboard.uid, true));
            lines.push(Line::default());
            lines.push(hint("Enter 下一步 · Esc 返回", inner.width));
        }
        OnboardStep::TypeSelect => {
            lines.push(section("账户类型", inner.width));
            for (t, desc) in [
                ("Individual", "个人账户（普通玩家，默认）"),
                ("Bank", "银行账户（成员银行，需 AEU 认证）"),
                ("Country", "国家账户（主权国家，理事会登记）"),
            ] {
                let selected = app.onboard.atype.as_str() == t;
                let mark = if selected { "› " } else { "  " };
                lines.push(Line::from(vec![
                    Span::styled(mark, Style::default().fg(if selected { ACCENT } else { MUT })),
                    Span::styled(t, Style::default().fg(if selected { ACCENT } else { FG }).add_modifier(if selected { Modifier::BOLD } else { Modifier::empty() })),
                    Span::styled(format!("  — {desc}"), Style::default().fg(MUT)),
                ]));
            }
            lines.push(Line::default());
            lines.push(hint("↑ ↓ 选择 · Enter 确认", inner.width));
        }
        OnboardStep::Email => {
            lines.push(section("联系邮箱", inner.width));
            lines.push(field_label("邮箱（同时作为 gpg 密钥身份）", "如 Steve@aeu.org", true));
            lines.push(value_line(&app.onboard.email, true));
            lines.push(Line::default());
            lines.push(hint("Enter 下一步 · Esc 返回", inner.width));
        }
        OnboardStep::Passphrase => {
            lines.push(section("设置钱包口令", inner.width));
            lines.push(field_label("口令（≥8 位，用于本地签名与解锁，遗忘不可找回）", "", true));
            lines.push(value_line(&mask(&app.onboard.pass1), true));
            lines.push(Line::default());
            lines.push(hint("Enter 下一步 · Esc 返回", inner.width));
        }
        OnboardStep::ConfirmPass => {
            lines.push(section("确认口令", inner.width));
            lines.push(field_label("再次输入口令", "", true));
            lines.push(value_line(&mask(&app.onboard.pass2), true));
            lines.push(Line::default());
            lines.push(hint("Enter 完成 · Esc 重新输入", inner.width));
        }
        OnboardStep::Generating => {
            lines.push(line_center(Span::styled("正在生成 ed25519 密钥（gpg）…", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)), inner.width));
            lines.push(line_center(Span::styled("请稍候", Style::default().fg(MUT)), inner.width));
        }
        OnboardStep::Done => {
            lines.push(line_center(Span::styled("✔ 引导完成！", Style::default().fg(OK).add_modifier(Modifier::BOLD)), inner.width));
            lines.push(Line::default());
            lines.push(line_center(Span::styled("密钥已生成，钱包已创建。", Style::default().fg(FG)), inner.width));
            lines.push(line_center(Span::styled("按 Enter 进入主界面", Style::default().fg(ACCENT)), inner.width));
        }
    }

    if let Some(e) = &app.onboard.error {
        lines.push(Line::default());
        lines.push(line_center(Span::styled(format!("✗ {e}"), Style::default().fg(ERR).add_modifier(Modifier::BOLD)), inner.width));
    }

    let p = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(p, inner);
}

// ---------------- 主界面 ----------------
fn draw_main(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // 顶部状态栏
            Constraint::Min(0),    // 中部
            Constraint::Length(3), // 输入栏
            Constraint::Length(1), // 帮助栏
        ])
        .split(area);

    draw_topbar(frame, app, rows[0]);
    draw_body(frame, app, rows[1]);
    draw_input(frame, app, rows[2]);
    draw_help(frame, app, rows[3]);
}

fn draw_topbar(frame: &mut Frame, app: &App, area: Rect) {
    let w = app.wallet.info.uid.clone();
    let atype = app.wallet.info.atype.as_str();
    let synced = if app.wallet.info.synced_at > 0 {
        format!("已同步 {}", ts(app.wallet.info.synced_at))
    } else {
        "未同步".to_string()
    };
    let mut spans = vec![
        Span::styled(" ● ", Style::default().fg(ACCENT2)),
        Span::styled("Alpha Wallet", Style::default().fg(FG).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" v{}", env!("CARGO_PKG_VERSION")), Style::default().fg(MUT)),
    ];
    spans.push(Span::raw("   "));
    spans.push(Span::styled(w, Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)));
    spans.push(Span::styled(format!(" · {atype}"), Style::default().fg(MUT)));
    let right_line = Line::from(vec![Span::styled(synced, Style::default().fg(MUT))]);
    frame.render_widget(Paragraph::new(Line::from(spans)), Rect { x: area.x + 1, y: area.y, width: area.width.saturating_sub(2), height: 1 });
    // 用 Line::width() 计算真实显示宽度（中文按 2 列），避免“未同步”被截断成“未同”
    let right_w = right_line.width() as u16;
    if area.width >= right_w + 4 {
        let right_area = Rect { x: area.x + area.width - right_w - 1, y: area.y, width: right_w + 1, height: 1 };
        frame.render_widget(Paragraph::new(right_line), right_area);
    }
    let line = Paragraph::new(Line::from(vec![Span::styled("─".repeat(area.width as usize), Style::default().fg(BORDER))]));
    frame.render_widget(line, Rect { x: area.x, y: area.y + 2, width: area.width, height: 1 });
}

fn draw_body(frame: &mut Frame, app: &App, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(14), Constraint::Min(0)])
        .split(area);
    draw_nav(frame, app, cols[0]);
    draw_content(frame, app, cols[1]);
}

fn draw_nav(frame: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = View::ALL
        .iter()
        .map(|v| {
            let selected = *v == app.view;
            let prefix = if selected { "▍" } else { " " };
            ListItem::new(Line::from(vec![
                Span::styled(prefix, Style::default().fg(if selected { ACCENT } else { MUT })),
                Span::styled(
                    v.title(),
                    Style::default().fg(if selected { ACCENT } else { MUT })
                        .add_modifier(if selected { Modifier::BOLD } else { Modifier::empty() }),
                ),
            ]))
        })
        .collect();
    let mut list = List::new(items).block(
        Block::default()
            .borders(Borders::RIGHT)
            .border_style(Style::default().fg(BORDER)),
    );
    let mut state = ListState::default();
    state.select(Some(app.view.idx()));
    list = list.highlight_style(Style::default().bg(PANEL));
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_content(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(BORDER))
        .title(Span::styled(format!(" {} ", app.view.title()), Style::default().fg(ACCENT)));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if app.help_visible {
        draw_help_panel(frame, inner);
        return;
    }
    match app.view {
        View::Overview => draw_overview(frame, app, inner),
        View::Transactions => draw_transactions(frame, app, inner),
        View::Accounts => draw_accounts(frame, app, inner),
        View::Outbox => draw_outbox(frame, app, inner),
        View::Settings => draw_settings(frame, app, inner),
    }
}

/// 多行帮助面板（内容区展示，避免单行溢出）。
fn draw_help_panel(frame: &mut Frame, area: Rect) {
    let rows: &[&str] = &[
        "导航    ↑ / ↓          切换视图（总览/交易/账户/待提交/设置）",
        "        1-5            直接跳到对应视图",
        "同步    r              从中心/镜像同步账本",
        "命令    : 或 /          打开命令输入",
        "        help / ?       显示本帮助",
        "开立    :open          在中心开立账户",
        "转账    :send <uid[@类型]> <金额>   本地签名转账（写入 outbox）",
        "        :submit [tx_id]   提交待确认交易",
        "        :confirm [tx_id]  确认接收的交易",
        "设置    :set server <地址>    设置中心服务器",
        "        :set apikey <key>    设置镜像 apikey（可选）",
        "退出    q / quit       退出程序",
        "        Esc            关闭本帮助 / 取消输入",
    ];
    let lines: Vec<Line> = rows
        .iter()
        .map(|s| {
            Line::from(vec![Span::styled(*s, Style::default().fg(FG))])
        })
        .collect();
    let p = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(p, area);
}

fn draw_overview(frame: &mut Frame, app: &App, area: Rect) {
    let balance = crate::txn::balance(&app.wallet);
    let mut lines = Vec::new();
    lines.push(Line::from(vec![Span::styled("账户余额（镜像口径）", Style::default().fg(MUT))]));
    lines.push(Line::from(vec![
        Span::styled(format!("{balance}"), Style::default().fg(FG).add_modifier(Modifier::BOLD).add_modifier(Modifier::UNDERLINED)),
        Span::styled("  A€", Style::default().fg(ACCENT)),
    ]));
    lines.push(Line::default());
    lines.push(Line::from(vec![Span::styled(format!("UID：{}", app.wallet.info.uid), Style::default().fg(FG))]));
    lines.push(Line::from(vec![Span::styled(format!("类型：{}", app.wallet.info.atype.as_str()), Style::default().fg(FG))]));
    lines.push(Line::from(vec![Span::styled(format!("中心：{}", if app.wallet.info.server_url.is_empty() { "未配置" } else { &app.wallet.info.server_url }), Style::default().fg(FG))]));
    lines.push(Line::default());
    if let Some(r) = &app.sync_res {
        lines.push(Line::from(vec![Span::styled("最近同步", Style::default().fg(OK).add_modifier(Modifier::BOLD))]));
        lines.push(Line::from(vec![Span::styled(format!("  新增交易 {} · 账户快照 {} · 快照哈希 {:.16}…", r.txs, r.accounts, r.hash), Style::default().fg(FG))]));
    } else {
        lines.push(Line::from(vec![Span::styled("尚未同步。按 r 从中心镜像拉取账本。", Style::default().fg(WARN))]));
    }
    lines.push(Line::default());
    lines.push(Line::from(vec![Span::styled("快捷键：", Style::default().fg(MUT))]));
    lines.push(Line::from(vec![Span::styled("  r 同步 · t 转账（或输入 send <uid> <金额>）· ↑↓/数字 切视图 · q 退出", Style::default().fg(MUT))]));
    let p = Paragraph::new(lines).wrap(Wrap { trim: true });
    frame.render_widget(p, area);
}

fn draw_transactions(frame: &mut Frame, app: &App, area: Rect) {
    if app.txs.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::styled("暂无相关交易。同步后可见。", Style::default().fg(MUT))])),
            area,
        );
        return;
    }
    let header = vec![
        Span::styled(format!("{:<20} ", "时间"), Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::styled(format!("{:<10} ", "类型"), Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::styled(format!("{:<24} ", "对方"), Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::styled(format!("{:>10} ", "金额"), Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::styled(format!("{:<9}", "状态"), Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
    ];
    let mut items = vec![ListItem::new(Line::from(header))];
    for (tx_id, tx_type, peer, _pt, amount, tsv, status) in &app.txs {
        let _ = tx_id;
        let color = match status.as_str() {
            "Confirmed" => OK,
            "Rejected" => ERR,
            _ => WARN,
        };
        items.push(ListItem::new(Line::from(vec![
            Span::raw(format!("{:<20} ", ts(*tsv))),
            Span::raw(format!("{:<10} ", tx_type)),
            Span::raw(format!("{:<24} ", peer)),
            Span::styled(format!("{:>10} ", amount), Style::default().fg(FG)),
            Span::styled(format!("{:<9}", status), Style::default().fg(color).add_modifier(Modifier::BOLD)),
        ])));
    }
    let list = List::new(items).block(Block::default().style(Style::default().bg(BG)));
    frame.render_widget(list, area);
}

fn draw_accounts(frame: &mut Frame, app: &App, area: Rect) {
    if app.accounts.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::styled("尚无账户快照。按 r 同步后可见中心账户。", Style::default().fg(MUT))])),
            area,
        );
        return;
    }
    let mut items = vec![ListItem::new(Line::from(vec![
        Span::styled(format!("{:<24} ", "UID"), Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::styled(format!("{:<12} ", "类型"), Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::styled(format!("{:>12} ", "余额"), Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::styled(format!("{:<9}", "状态"), Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
    ]))];
    for (uid, atype, balance, status) in &app.accounts {
        let mark = if *uid == app.wallet.info.uid { "◉" } else { " " };
        items.push(ListItem::new(Line::from(vec![
            Span::raw(format!("{mark:<1}")),
            Span::raw(format!("{:<23} ", uid)),
            Span::raw(format!("{:<12} ", atype)),
            Span::raw(format!("{:>12} ", balance)),
            Span::styled(status.clone(), Style::default().fg(if status == "Active" { OK } else { WARN })),
        ])));
    }
    let list = List::new(items).block(Block::default().style(Style::default().bg(BG)));
    frame.render_widget(list, area);
}

fn draw_outbox(frame: &mut Frame, app: &App, area: Rect) {
    if app.outbox.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::styled("outbox 为空。发送转账后，签名交易会出现在这里待提交。", Style::default().fg(MUT))])),
            area,
        );
        return;
    }
    let mut items = Vec::new();
    for (tx_id, state, created) in &app.outbox {
        items.push(ListItem::new(Line::from(vec![
            Span::raw(format!("{:<34} ", tx_id)),
            Span::styled(state.clone(), Style::default().fg(if state == "Pending" { WARN } else { OK })),
            Span::raw(format!("  {}", ts(*created))),
        ])));
    }
    let list = List::new(items).block(Block::default().style(Style::default().bg(BG)));
    frame.render_widget(list, area);
}

fn draw_settings(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines = Vec::new();
    lines.push(Line::from(vec![Span::styled("连接", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))]));
    lines.push(Line::from(vec![Span::styled(format!("  中心地址：{}", app.wallet.info.server_url), Style::default().fg(FG))]));
    lines.push(Line::from(vec![Span::styled(format!("  镜像 apikey：{}", mask(&app.wallet.info.mirror_apikey)), Style::default().fg(FG))]));
    lines.push(Line::default());
    lines.push(Line::from(vec![Span::styled("钱包", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))]));
    lines.push(Line::from(vec![Span::styled(format!("  UID：{}", app.wallet.info.uid), Style::default().fg(FG))]));
    lines.push(Line::from(vec![Span::styled(format!("  类型：{}", app.wallet.info.atype.as_str()), Style::default().fg(FG))]));
    lines.push(Line::from(vec![Span::styled(format!("  邮箱：{}", app.wallet.info.email), Style::default().fg(FG))]));
    lines.push(Line::from(vec![Span::styled(format!("  数据目录：{}", crate::wallet::data_dir_str().display()), Style::default().fg(MUT))]));
    lines.push(Line::default());
    lines.push(Line::from(vec![Span::styled("说明", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))]));
    lines.push(Line::from(vec![Span::styled("  • 中心 > 本地 > 镜像：本地只读镜像账本，真实结算以中心为准。", Style::default().fg(MUT))]));
    lines.push(Line::from(vec![Span::styled("  • 修改配置：直接编辑 ~/.alpha_dir/alpha.db 或删除后重新引导。", Style::default().fg(MUT))]));
    lines.push(Line::from(vec![Span::styled("  • 口令遗忘不可找回，请妥善保管。", Style::default().fg(WARN))]));
    let p = Paragraph::new(lines).wrap(Wrap { trim: true });
    frame.render_widget(p, area);
}

fn draw_input(frame: &mut Frame, app: &App, area: Rect) {
    // 空闲时输入栏显示状态消息（命令反馈 / 同步结果）
    if app.input_mode == InputMode::None {
        let p = Paragraph::new(Line::from(vec![Span::styled(
            app.status.clone(),
            Style::default().fg(MUT),
        )]));
        frame.render_widget(p, Rect { x: area.x + 1, y: area.y, width: area.width.saturating_sub(2), height: 1 });
        return;
    }
    let (prompt, hidden): (&str, bool) = match app.input_mode {
        InputMode::Command => ("> ", false),
        InputMode::Passphrase => ("口令> ", true),
        InputMode::None => ("", false),
    };
    let mut text = String::from(prompt);
    let shown = if hidden { mask(&app.input) } else { app.input.clone() };
    text.push_str(&shown);
    if app.input_mode != InputMode::None {
        text.push('▌');
    }
    let style = if app.input_mode == InputMode::None {
        Style::default().fg(MUT)
    } else {
        Style::default().fg(FG).add_modifier(Modifier::BOLD)
    };
    let p = Paragraph::new(text).style(style);
    frame.render_widget(p, Rect { x: area.x + 1, y: area.y, width: area.width.saturating_sub(2), height: 1 });
}

fn draw_help(frame: &mut Frame, app: &App, area: Rect) {
    let msg = match app.input_mode {
        InputMode::Command => "Enter 执行 · Esc 取消",
        InputMode::Passphrase => "输入钱包口令签名 · Enter 确认 · Esc 取消",
        InputMode::None => {
            "↑↓ 切换 · 1-5 视图 · r 同步 · : 命令 · h 帮助 · q 退出"
        }
    };
    let p = Paragraph::new(Line::from(vec![Span::styled(msg, Style::default().fg(MUT))]));
    frame.render_widget(p, Rect { x: area.x + 1, y: area.y, width: area.width.saturating_sub(2), height: 1 });
}

// ---------------- 辅助 ----------------
fn line_center(span: Span<'static>, w: u16) -> Line<'static> {
    let len = span.content.chars().count() as u16;
    let pad = w.saturating_sub(len) / 2;
    Line::from(vec![Span::raw(" ".repeat(pad as usize)), span])
}

fn wrapped(text: &str, color: Color, w: u16) -> Line<'static> {
    let _ = w;
    Line::from(vec![Span::styled(text.to_string(), Style::default().fg(color))])
}

fn section(title: &str, _w: u16) -> Line<'static> {
    Line::from(vec![
        Span::styled("◆ ".to_string(), Style::default().fg(ACCENT2)),
        Span::styled(title.to_string(), Style::default().fg(FG).add_modifier(Modifier::BOLD)),
    ])
}

fn field_label(label: &str, hint: &str, _active: bool) -> Line<'static> {
    let mut spans = vec![Span::styled(label.to_string(), Style::default().fg(MUT))];
    if !hint.is_empty() {
        spans.push(Span::styled(format!("  {hint}"), Style::default().fg(BORDER)));
    }
    Line::from(spans)
}

fn value_line(value: &str, active: bool) -> Line<'static> {
    let arrow = if active { "▸ " } else { "  " };
    Line::from(vec![
        Span::styled(arrow, Style::default().fg(if active { ACCENT } else { MUT })),
        Span::styled(
            value.to_string(),
            Style::default().fg(if active { FG } else { MUT }).add_modifier(if active { Modifier::BOLD } else { Modifier::empty() }),
        ),
    ])
}

fn hint(text: &str, _w: u16) -> Line<'static> {
    Line::from(vec![Span::styled(text.to_string(), Style::default().fg(ACCENT))])
}

fn mask(s: &str) -> String {
    if s.is_empty() {
        String::new()
    } else {
        "•".repeat(s.chars().count())
    }
}

fn ts(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0)
        .map(|d| d.format("%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "-".into())
}

// 供 Frame 类型使用避免未用告警
pub fn _alignment() -> Alignment {
    Alignment::Left
}
