//! 帮助面板渲染回归测试：确认 wrap 渲染在不同窗口宽度下不 panic、不卡死。
use ratatui::{
    backend::TestBackend,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Terminal,
};

fn help_rows() -> &'static [&'static str] {
    &[
        "导航    ↑ / ↓     切换视图 · 1-5 直达",
        "同步    r         从中心/镜像同步",
        "命令    : 或 /     打开命令输入 · help/? 本帮助",
        "开立    :open     在中心开立账户",
        "转账    :send <uid[@类型]> <金额>   签名转账",
        "        :submit [tx_id]   提交待确认",
        "        :confirm [tx_id]  确认接收",
        "设置    :set server <地址>   设中心",
        "        :set apikey <key>   设镜像(可选)",
        "退出    q / quit   退出程序",
        "关闭    h / Esc    关闭本帮助",
    ]
}

fn render(width: u16, height: u16) {
    let backend = TestBackend::new(width, height);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| {
        let lines: Vec<Line> = help_rows()
            .iter()
            .map(|s| Line::from(vec![Span::styled(*s, Style::default().fg(Color::White))]))
            .collect();
        let p = Paragraph::new(lines).wrap(Wrap { trim: false });
        let inner_w = width.saturating_sub(4).max(1);
        let inner_h = height.saturating_sub(4).max(1);
        f.render_widget(p, Rect::new(1, 1, inner_w, inner_h));
    })
    .unwrap();
}

#[test]
fn help_panel_wide() {
    render(120, 30);
}

#[test]
fn help_panel_narrow() {
    // 窄窗口 + wrap，验证不 panic
    render(40, 30);
}

#[test]
fn help_panel_tiny() {
    // 极小窗口，验证不 panic
    render(30, 10);
    render(12, 3);
}
