// 两个 QT 客户端共用的界面工具（样式、格式化、消息框、类型白名单）。
#pragma once

#include <QString>
#include <QStringList>
#include <QWidget>

class QApplication;
class QComboBox;
class QTableWidget;

namespace acs {

/// 一个客户端允许的账户类型（D30：两客户端互相排斥）。
struct AppProfile {
    QString appTitle;       // 窗口标题
    QString appShort;       // 品牌短名（如 Alpha Wallet）
    QStringList types;      // 允许的账户类型（AccountType 字符串，按展示顺序）
    bool allowRegister;     // 是否允许自助注册
};

/// 深色主题 QSS（两个客户端共用；不依赖外部资源文件，单 exe 分发）。
/// 高对比度取色：正文近白、次要文字明显亮于背景、边框/强调色更清晰。
QString darkStyleSheet();

/// 给整个应用套主题：Fusion 基底（避免 Windows 11 原生控件把深色 QSS 顶掉）
/// + 调色板（占位符/禁用态/选中色）+ 全局 QSS。main() 里创建 QApplication 后立即调用。
void applyTheme(QApplication &app);

/// 账户类型 → 中文名。
QString typeLabel(const QString &type);

/// 把允许类型填进下拉框（显示中文名，userData 存英文类型）。
void fillTypeCombo(QComboBox *combo, const QStringList &types);

/// 金额格式化（整数 A€，千分位）。
QString fmtAmount(qint64 amount);

/// 时间戳（Unix 秒）→ 本地时间字符串。
QString fmtTime(qint64 ts);

/// 统一消息框（错误带标题）。
void showError(QWidget *parent, const QString &title, const QString &msg);
void showInfo(QWidget *parent, const QString &title, const QString &msg);

/// 用系统默认浏览器打开 http(s) 链接（协议/隐私网页由中心提供）。
bool openInBrowser(const QString &url);

/// 设置表格通用外观（整行选择、隔行变色、自适应列宽）。
void styleTable(QTableWidget *table);

} // namespace acs
