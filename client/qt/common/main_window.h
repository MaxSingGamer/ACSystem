// 主窗口（两个客户端共用）：海军蓝品牌条 + 左侧竖向导航 + 右侧内容卡片（Citi 风）。
#pragma once

#include <QMainWindow>

#include "ui_common.h"

class QLabel;
class QStackedWidget;
class QTableWidget;
class QComboBox;
class QLineEdit;
class QPushButton;
class QButtonGroup;
class QParallelAnimationGroup;
class QVariantAnimation;

class MainWindow : public QMainWindow
{
    Q_OBJECT
public:
    explicit MainWindow(const acs::AppProfile &profile, QWidget *parent = nullptr);

    /// 登录成功后进入：刷新状态并自动同步一次。
    void onLoggedIn();

private slots:
    void refreshState();
    void doSync();
    void doTransfer();
    void loadPending();
    void loadLedger();
    void confirmSelected();
    void rejectSelected();
    void doLogout();
    void openPrivacy();

private:
    QWidget *buildHeader();
    QWidget *buildSidebar();
    QWidget *pageOverview();
    QWidget *pageTransfer();
    QWidget *pageInbox();
    QWidget *pageLedger();
    QWidget *pageAbout();
    void setPage(int index);
    /// 页面切换动画：淡入 + 轻微横向滑入（180~200ms，OutCubic）。
    void animatePageSwitch(int from, int to);
    /// 余额数字滚动动画（400ms；金额变化时从旧值平滑过渡到新值）。
    void setBalanceAnimated(qint64 target);
    QString selectedPendingTxId() const;

    acs::AppProfile m_profile;
    QLabel *m_brand = nullptr;
    QLabel *m_chip = nullptr;
    QLabel *m_balance = nullptr;
    QLabel *m_status = nullptr;
    QLabel *m_overviewHint = nullptr;
    QStackedWidget *m_pages = nullptr;
    QButtonGroup *m_nav = nullptr;
    QTableWidget *m_pending = nullptr;
    QTableWidget *m_ledger = nullptr;
    QComboBox *m_toType = nullptr;
    QLineEdit *m_toUid = nullptr;
    QLineEdit *m_amount = nullptr;
    QLineEdit *m_transferPass = nullptr;
    bool m_ready = false;              // 构造期不播动画
    qint64 m_balanceShown = 0;         // 当前显示的余额（用于滚动动画起点）
    QVariantAnimation *m_balanceAnim = nullptr;
    QParallelAnimationGroup *m_pageAnim = nullptr;
};
