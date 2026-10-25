#include "ui_common.h"

#include <QApplication>
#include <QComboBox>
#include <QDateTime>
#include <QDesktopServices>
#include <QHeaderView>
#include <QLocale>
#include <QMessageBox>
#include <QTableWidget>
#include <QUrl>

namespace acs {

// ---------------------------------------------------------------------------
// 配色：参考花旗银行（Citi）的「白色卡片 + 深海军蓝 + 品牌蓝/红点缀」金融风格。
//   navy  #003B70（主色：标题/大数字/品牌条）
//   blue  #056DAE（品牌蓝：主按钮/选中/链接）
//   red   #E60028（Citi 红：危险操作）
//   bg    #F4F6FA   card #FFFFFF   line #DDE4EE
//   text  #101C2E   sub  #5B6B85
// ---------------------------------------------------------------------------
QString darkStyleSheet()
{
    return QStringLiteral(R"(
* { font-family: "Microsoft YaHei UI","Segoe UI",sans-serif; font-size: 13.5px; }
QWidget { background: #F4F6FA; color: #101C2E; }
QMainWindow, QDialog { background: #F4F6FA; }
QLabel { color: #101C2E; background: transparent; }
QLabel#muted, QLabel.muted { color: #5B6B85; }
QLabel#title { font-size: 20px; font-weight: 800; color: #003B70; }
QLabel#big { font-size: 34px; font-weight: 800; color: #003B70; }

QWidget#headerBar { background: #003B70; }
QLabel#headerBrand { color: #FFFFFF; font-size: 17px; font-weight: 800; background: transparent; }
QLabel#headerSub { color: #BFD4EA; font-size: 12.5px; background: transparent; }
QPushButton#headerBtn {
    background: rgba(255,255,255,0.14); color: #FFFFFF; border: 1px solid rgba(255,255,255,0.40);
    border-radius: 8px; padding: 7px 14px; font-weight: 600;
}
QPushButton#headerBtn:hover { background: rgba(255,255,255,0.26); }

QGroupBox, QFrame#card {
    background: #FFFFFF; border: 1px solid #DDE4EE; border-radius: 14px;
    margin-top: 16px; padding: 14px 12px 10px 12px;
}
QGroupBox::title { subcontrol-origin: margin; left: 14px; padding: 0 6px; color: #5B6B85; font-weight: 700; }
QGroupBox QLabel, QFrame#card QLabel { background: transparent; }

QPushButton#nav {
    background: transparent; color: #3B4B66; border: none; border-radius: 10px;
    padding: 11px 14px; text-align: left; font-weight: 600;
}
QPushButton#nav:hover { background: #E8EEF7; color: #003B70; }
QPushButton#nav:checked { background: #E4EEFA; color: #003B70; border-left: 3px solid #056DAE; }

QLineEdit, QComboBox, QSpinBox {
    background: #FFFFFF; color: #101C2E; border: 1px solid #C9D4E3; border-radius: 9px;
    padding: 9px 12px; selection-background-color: #056DAE; selection-color: #FFFFFF;
}
QLineEdit:focus, QComboBox:focus, QSpinBox:focus { border: 2px solid #056DAE; padding: 8px 11px; }
QLineEdit:disabled, QComboBox:disabled { background: #F1F4F9; color: #9AA7BC; }
QComboBox::drop-down { border: none; width: 24px; }
QComboBox QAbstractItemView {
    background: #FFFFFF; color: #101C2E; border: 1px solid #C9D4E3;
    selection-background-color: #E4EEFA; selection-color: #003B70; outline: none;
}

QPushButton {
    background: #FFFFFF; color: #003B70; border: 1px solid #C9D4E3;
    border-radius: 9px; padding: 9px 18px; font-weight: 600;
}
QPushButton:hover { background: #F1F6FC; border-color: #056DAE; }
QPushButton:pressed { background: #E4EEFA; }
QPushButton:disabled { background: #F1F4F9; color: #A7B3C6; border-color: #E2E8F0; }
QPushButton#primary { background: #056DAE; color: #FFFFFF; border-color: #056DAE; font-weight: 700; }
QPushButton#primary:hover { background: #0A7BC4; border-color: #0A7BC4; }
QPushButton#danger { background: #FFFFFF; color: #E60028; border-color: #F3B6C1; }
QPushButton#danger:hover { background: #FDECEF; border-color: #E60028; }
QPushButton#link { background: transparent; border: none; color: #056DAE; padding: 4px 2px; font-weight: 600; }
QPushButton#link:hover { color: #0A7BC4; text-decoration: underline; }

QTableWidget {
    background: #FFFFFF; color: #101C2E; border: 1px solid #DDE4EE; border-radius: 12px;
    gridline-color: #EDF1F7; alternate-background-color: #FAFBFD;
    selection-background-color: #E4EEFA; selection-color: #003B70;
}
QTableWidget::item { padding: 7px; }
QHeaderView::section {
    background: #F7F9FC; color: #5B6B85; border: none; border-bottom: 2px solid #E6ECF5;
    padding: 10px 8px; font-weight: 700;
}

QCheckBox { color: #101C2E; spacing: 8px; background: transparent; }
QCheckBox::indicator { width: 18px; height: 18px; border: 1.5px solid #B6C3D6; border-radius: 5px; background: #FFFFFF; }
QCheckBox::indicator:checked { background: #056DAE; border-color: #056DAE; }
QListWidget { background: #FFFFFF; color: #101C2E; border: 1px solid #DDE4EE; border-radius: 12px; outline: none; }
QListWidget::item { padding: 12px; border-bottom: 1px solid #EEF2F8; }
QListWidget::item:selected { background: #E4EEFA; color: #003B70; }
QListWidget::item:hover { background: #F5F8FD; }
QStatusBar { color: #5B6B85; background: #FFFFFF; border-top: 1px solid #E6ECF5; }
QToolTip { background: #003B70; color: #FFFFFF; border: none; padding: 6px; }
QScrollArea { border: none; background: transparent; }
QScrollBar:vertical { background: #F4F6FA; width: 12px; margin: 0; }
QScrollBar::handle:vertical { background: #C9D4E3; border-radius: 6px; min-height: 30px; }
QScrollBar::handle:vertical:hover { background: #056DAE; }
QScrollBar::add-line, QScrollBar::sub-line { height: 0; }
QTabWidget::pane { border: 1px solid #DDE4EE; border-radius: 14px; background: #FFFFFF; top: -1px; }
QTabBar::tab { background: transparent; color: #5B6B85; padding: 10px 20px; font-weight: 600; border-bottom: 3px solid transparent; }
QTabBar::tab:selected { color: #003B70; border-bottom: 3px solid #056DAE; }
QTabBar::tab:hover:!selected { color: #056DAE; }
)");
}

void applyTheme(QApplication &app)
{
    // Fusion 基底：Windows 11 原生样式会忽略部分 QSS（输入框/下拉仍是系统色），
    // 用 Fusion 才能保证整套金融风配色在各控件上一致。
    QApplication::setStyle(QStringLiteral("Fusion"));

    QPalette pal;
    pal.setColor(QPalette::Window, QColor(QStringLiteral("#F4F6FA")));
    pal.setColor(QPalette::WindowText, QColor(QStringLiteral("#101C2E")));
    pal.setColor(QPalette::Base, QColor(QStringLiteral("#FFFFFF")));
    pal.setColor(QPalette::AlternateBase, QColor(QStringLiteral("#FAFBFD")));
    pal.setColor(QPalette::Text, QColor(QStringLiteral("#101C2E")));
    pal.setColor(QPalette::PlaceholderText, QColor(QStringLiteral("#9AA7BC")));
    pal.setColor(QPalette::Button, QColor(QStringLiteral("#FFFFFF")));
    pal.setColor(QPalette::ButtonText, QColor(QStringLiteral("#003B70")));
    pal.setColor(QPalette::Highlight, QColor(QStringLiteral("#056DAE")));
    pal.setColor(QPalette::HighlightedText, QColor(QStringLiteral("#FFFFFF")));
    pal.setColor(QPalette::ToolTipBase, QColor(QStringLiteral("#003B70")));
    pal.setColor(QPalette::ToolTipText, QColor(QStringLiteral("#FFFFFF")));
    pal.setColor(QPalette::Disabled, QPalette::Text, QColor(QStringLiteral("#A7B3C6")));
    pal.setColor(QPalette::Disabled, QPalette::ButtonText, QColor(QStringLiteral("#A7B3C6")));
    pal.setColor(QPalette::Disabled, QPalette::WindowText, QColor(QStringLiteral("#A7B3C6")));
    app.setPalette(pal);
    app.setStyleSheet(darkStyleSheet());
}

QString typeLabel(const QString &type)
{
    if (type == QStringLiteral("Individual"))
        return QStringLiteral("个人");
    if (type == QStringLiteral("Country"))
        return QStringLiteral("国家");
    if (type == QStringLiteral("Company"))
        return QStringLiteral("企业 / 银行");
    if (type == QStringLiteral("System"))
        return QStringLiteral("系统");
    return type;
}

void fillTypeCombo(QComboBox *combo, const QStringList &types)
{
    combo->clear();
    for (const QString &t : types) {
        combo->addItem(typeLabel(t), t);
    }
}

QString fmtAmount(qint64 amount)
{
    return QLocale(QLocale::Chinese, QLocale::China).toString(amount);
}

QString fmtTime(qint64 ts)
{
    if (ts <= 0)
        return QStringLiteral("—");
    return QDateTime::fromSecsSinceEpoch(ts).toString(QStringLiteral("yyyy-MM-dd HH:mm:ss"));
}

void showError(QWidget *parent, const QString &title, const QString &msg)
{
    QMessageBox box(QMessageBox::Warning, title, msg, QMessageBox::Ok, parent);
    box.exec();
}

void showInfo(QWidget *parent, const QString &title, const QString &msg)
{
    QMessageBox box(QMessageBox::Information, title, msg, QMessageBox::Ok, parent);
    box.exec();
}

bool openInBrowser(const QString &url)
{
    if (!url.startsWith(QStringLiteral("http://")) && !url.startsWith(QStringLiteral("https://")))
        return false;
    return QDesktopServices::openUrl(QUrl(url));
}

void styleTable(QTableWidget *table)
{
    table->setSelectionBehavior(QAbstractItemView::SelectRows);
    table->setSelectionMode(QAbstractItemView::SingleSelection);
    table->setEditTriggers(QAbstractItemView::NoEditTriggers);
    table->setAlternatingRowColors(true);
    table->verticalHeader()->setVisible(false);
    table->horizontalHeader()->setStretchLastSection(true);
    table->horizontalHeader()->setSectionResizeMode(QHeaderView::ResizeToContents);
}

} // namespace acs
