#include "main_window.h"

#include "acs_abi.h"

#include <QApplication>
#include <QButtonGroup>
#include <QColor>
#include <QComboBox>
#include <QFormLayout>
#include <QFrame>
#include <QGraphicsOpacityEffect>
#include <QHBoxLayout>
#include <QHeaderView>
#include <QInputDialog>
#include <QJsonArray>
#include <QJsonObject>
#include <QLabel>
#include <QLineEdit>
#include <QParallelAnimationGroup>
#include <QPropertyAnimation>
#include <QPushButton>
#include <QStackedWidget>
#include <QStatusBar>
#include <QTableWidget>
#include <QVariantAnimation>
#include <QVBoxLayout>

namespace {
QFrame *card(const QString &title, QWidget *parent, QVBoxLayout **inner = nullptr)
{
    auto *f = new QFrame(parent);
    f->setObjectName(QStringLiteral("card"));
    auto *v = new QVBoxLayout(f);
    v->setContentsMargins(18, 14, 18, 16);
    v->setSpacing(10);
    if (!title.isEmpty()) {
        auto *t = new QLabel(title, f);
        t->setStyleSheet(QStringLiteral("color:#5B6B85;font-weight:700;font-size:12.5px;letter-spacing:1px;"));
        v->addWidget(t);
    }
    if (inner)
        *inner = v;
    return f;
}
} // namespace

MainWindow::MainWindow(const acs::AppProfile &profile, QWidget *parent)
    : QMainWindow(parent)
    , m_profile(profile)
{
    setWindowTitle(profile.appTitle);
    resize(1180, 780);
    setMinimumSize(980, 660);

    auto *central = new QWidget(this);
    auto *root = new QVBoxLayout(central);
    root->setContentsMargins(0, 0, 0, 0);
    root->setSpacing(0);

    root->addWidget(buildHeader());

    auto *body = new QHBoxLayout();
    body->setContentsMargins(0, 0, 0, 0);
    body->setSpacing(0);
    body->addWidget(buildSidebar());

    m_pages = new QStackedWidget(central);
    m_pages->addWidget(pageOverview());
    m_pages->addWidget(pageTransfer());
    m_pages->addWidget(pageInbox());
    m_pages->addWidget(pageLedger());
    m_pages->addWidget(pageAbout());
    auto *pageWrap = new QWidget(central);
    auto *pw = new QVBoxLayout(pageWrap);
    pw->setContentsMargins(22, 18, 22, 18);
    pw->addWidget(m_pages);
    body->addWidget(pageWrap, 1);
    root->addLayout(body, 1);

    setCentralWidget(central);
    statusBar()->showMessage(QStringLiteral("就绪"));
    setPage(0);
    m_ready = true;
}

QWidget *MainWindow::buildHeader()
{
    auto *bar = new QWidget(this);
    bar->setObjectName(QStringLiteral("headerBar"));
    bar->setFixedHeight(66);
    auto *h = new QHBoxLayout(bar);
    h->setContentsMargins(22, 0, 18, 0);
    h->setSpacing(12);

    auto *v = new QVBoxLayout();
    v->setSpacing(0);
    m_brand = new QLabel(m_profile.appTitle, bar);
    m_brand->setObjectName(QStringLiteral("headerBrand"));
    auto *sub = new QLabel(QStringLiteral("A€ 经济系统 · 客户端"), bar);
    sub->setObjectName(QStringLiteral("headerSub"));
    v->addWidget(m_brand);
    v->addWidget(sub);
    h->addLayout(v);
    h->addStretch(1);

    m_chip = new QLabel(QStringLiteral("未登录"), bar);
    m_chip->setStyleSheet(QStringLiteral("color:#FFFFFF;background:rgba(255,255,255,0.14);"
                                         "border-radius:12px;padding:6px 14px;font-weight:600;"));
    h->addWidget(m_chip);

    auto mk = [&](const QString &text, auto slot) {
        auto *b = new QPushButton(text, bar);
        b->setObjectName(QStringLiteral("headerBtn"));
        b->setCursor(Qt::PointingHandCursor);
        connect(b, &QPushButton::clicked, this, slot);
        h->addWidget(b);
    };
    mk(QStringLiteral("刷新"), &MainWindow::refreshState);
    mk(QStringLiteral("同步"), &MainWindow::doSync);
    mk(QStringLiteral("退出"), &MainWindow::doLogout);
    return bar;
}

QWidget *MainWindow::buildSidebar()
{
    auto *side = new QWidget(this);
    side->setFixedWidth(208);
    side->setStyleSheet(QStringLiteral("background:#FFFFFF;border-right:1px solid #E6ECF5;"));
    auto *v = new QVBoxLayout(side);
    v->setContentsMargins(14, 18, 14, 18);
    v->setSpacing(6);

    auto *cap = new QLabel(QStringLiteral("功能导航"), side);
    cap->setStyleSheet(QStringLiteral("color:#9AA7BC;font-size:12px;font-weight:700;letter-spacing:1px;"));
    v->addWidget(cap);

    m_nav = new QButtonGroup(this);
    m_nav->setExclusive(true);
    const QStringList names{QStringLiteral("概览"), QStringLiteral("转账"), QStringLiteral("待收箱"),
                            QStringLiteral("流水"), QStringLiteral("关于")};
    for (int i = 0; i < names.size(); ++i) {
        auto *b = new QPushButton(names.at(i), side);
        b->setObjectName(QStringLiteral("nav"));
        b->setCheckable(true);
        b->setCursor(Qt::PointingHandCursor);
        // 导航图标（内置标准图标，后续可替换为品牌 SVG）
        static const QStyle::StandardPixmap kIcons[] = {
            QStyle::SP_DesktopIcon, QStyle::SP_ArrowRight, QStyle::SP_MessageBoxInformation,
            QStyle::SP_FileDialogListView, QStyle::SP_MessageBoxQuestion};
        b->setIcon(style()->standardIcon(kIcons[i]));
        b->setIconSize(QSize(18, 18));
        m_nav->addButton(b, i);
        v->addWidget(b);
    }
    v->addStretch(1);
    auto *priv = new QPushButton(QStringLiteral("隐私政策"), side);
    priv->setObjectName(QStringLiteral("link"));
    connect(priv, &QPushButton::clicked, this, &MainWindow::openPrivacy);
    v->addWidget(priv);
    auto *ver = new QLabel(QStringLiteral("v") + Acs::version(), side);
    ver->setObjectName(QStringLiteral("muted"));
    v->addWidget(ver);

    connect(m_nav, &QButtonGroup::idClicked, this, &MainWindow::setPage);
    return side;
}

void MainWindow::setPage(int index)
{
    const int from = m_pages->currentIndex();
    m_pages->setCurrentIndex(index);
    if (auto *b = m_nav->button(index))
        b->setChecked(true);
    if (m_ready && from >= 0 && from != index)
        animatePageSwitch(from, index);
    if (index == 2)
        loadPending();
    else if (index == 3)
        loadLedger();
}

// 页面切换动画（v3.3.0 界面动效）：
// 新页面淡入（0→1，180ms）+ 从 28px 侧向滑入（200ms），OutCubic 曲线。
// 用 QStackedWidget 当前页的 pos 做位移：切换瞬间布局会把它放回 (0,0)，
// 随后动画接管，结束不影响布局。
void MainWindow::animatePageSwitch(int from, int to)
{
    QWidget *page = m_pages->widget(to);
    if (!page)
        return;
    if (m_pageAnim) {
        m_pageAnim->stop();
        m_pageAnim->deleteLater();
        m_pageAnim = nullptr;
    }
    const int dir = (to > from) ? 1 : -1;

    auto *eff = new QGraphicsOpacityEffect(page);
    page->setGraphicsEffect(eff);
    auto *fade = new QPropertyAnimation(eff, "opacity", page);
    fade->setDuration(180);
    fade->setStartValue(0.0);
    fade->setEndValue(1.0);
    fade->setEasingCurve(QEasingCurve::OutCubic);

    auto *slide = new QPropertyAnimation(page, "pos", page);
    slide->setDuration(210);
    slide->setStartValue(QPoint(26 * dir, 0));
    slide->setEndValue(QPoint(0, 0));
    slide->setEasingCurve(QEasingCurve::OutCubic);

    auto *group = new QParallelAnimationGroup(page);
    group->addAnimation(fade);
    group->addAnimation(slide);
    connect(group, &QParallelAnimationGroup::finished, page, [page, eff] {
        // 动画结束清除效果，避免影响后续绘制/子控件交互性能
        page->setGraphicsEffect(nullptr);
        eff->deleteLater();
    });
    m_pageAnim = group;
    group->start(QAbstractAnimation::DeleteWhenStopped);
}

// 余额数字滚动：从当前显示值平滑过渡到新值（带千分位），像银行 App 的数字动效。
void MainWindow::setBalanceAnimated(qint64 target)
{
    if (m_balanceAnim) {
        m_balanceAnim->stop();
        m_balanceAnim->deleteLater();
        m_balanceAnim = nullptr;
    }
    if (!m_balance || m_balanceShown == target) {
        m_balanceShown = target;
        if (m_balance)
            m_balance->setText(QStringLiteral("%1 A€").arg(acs::fmtAmount(target)));
        return;
    }
    auto *anim = new QVariantAnimation(this);
    anim->setDuration(420);
    anim->setStartValue(m_balanceShown);
    anim->setEndValue(target);
    anim->setEasingCurve(QEasingCurve::OutCubic);
    connect(anim, &QVariantAnimation::valueChanged, this, [this](const QVariant &v) {
        m_balanceShown = v.toLongLong();
        m_balance->setText(QStringLiteral("%1 A€").arg(acs::fmtAmount(m_balanceShown)));
    });
    m_balanceAnim = anim;
    anim->start(QAbstractAnimation::DeleteWhenStopped);
}

QWidget *MainWindow::pageOverview()
{
    auto *page = new QWidget(this);
    auto *v = new QVBoxLayout(page);
    v->setContentsMargins(0, 0, 0, 0);
    v->setSpacing(16);

    QVBoxLayout *bal = nullptr;
    auto *balCard = card(QStringLiteral("可用余额"), page, &bal);
    m_balance = new QLabel(QStringLiteral("—"), balCard);
    m_balance->setStyleSheet(QStringLiteral("color:#003B70;font-size:38px;font-weight:800;"));
    bal->addWidget(m_balance);
    m_overviewHint = new QLabel(QStringLiteral("A€（Alpha Coin）· 以中心账本为准"), balCard);
    m_overviewHint->setObjectName(QStringLiteral("muted"));
    bal->addWidget(m_overviewHint);
    v->addWidget(balCard);

    QVBoxLayout *st = nullptr;
    auto *stCard = card(QStringLiteral("账户状态"), page, &st);
    m_status = new QLabel(QStringLiteral("—"), stCard);
    m_status->setWordWrap(true);
    st->addWidget(m_status);
    v->addWidget(stCard);
    v->addStretch(1);
    return page;
}

QWidget *MainWindow::pageTransfer()
{
    auto *page = new QWidget(this);
    auto *v = new QVBoxLayout(page);
    v->setContentsMargins(0, 0, 0, 0);

    QVBoxLayout *inner = nullptr;
    auto *c = card(QStringLiteral("发起转账"), page, &inner);
    auto *form = new QFormLayout();
    form->setHorizontalSpacing(18);
    form->setVerticalSpacing(14);
    m_toType = new QComboBox(c);
    acs::fillTypeCombo(m_toType,
                       {QStringLiteral("Individual"), QStringLiteral("Country"),
                        QStringLiteral("Company"), QStringLiteral("System")});
    m_toUid = new QLineEdit(c);
    m_toUid->setPlaceholderText(QStringLiteral("接收方 UID / 简称"));
    m_amount = new QLineEdit(c);
    m_amount->setPlaceholderText(QStringLiteral("金额（正整数 A€）"));
    m_transferPass = new QLineEdit(c);
    m_transferPass->setEchoMode(QLineEdit::Password);
    m_transferPass->setPlaceholderText(QStringLiteral("钱包口令（仅用于本地签名）"));
    form->addRow(QStringLiteral("接收方类型"), m_toType);
    form->addRow(QStringLiteral("接收方账户"), m_toUid);
    form->addRow(QStringLiteral("金额"), m_amount);
    form->addRow(QStringLiteral("口令"), m_transferPass);
    inner->addLayout(form);

    auto *row = new QHBoxLayout();
    auto *btn = new QPushButton(QStringLiteral("提交转账"), c);
    btn->setObjectName(QStringLiteral("primary"));
    btn->setMinimumHeight(40);
    auto *tip = new QLabel(QStringLiteral("接收方接受后才入账；拒收则自动退回。"), c);
    tip->setObjectName(QStringLiteral("muted"));
    row->addWidget(btn, 1);
    row->addWidget(tip);
    inner->addLayout(row);
    connect(btn, &QPushButton::clicked, this, &MainWindow::doTransfer);

    v->addWidget(c);
    v->addStretch(1);
    return page;
}

QWidget *MainWindow::pageInbox()
{
    auto *page = new QWidget(this);
    auto *v = new QVBoxLayout(page);
    v->setContentsMargins(0, 0, 0, 0);

    QVBoxLayout *inner = nullptr;
    auto *c = card(QStringLiteral("待收箱"), page, &inner);
    m_pending = new QTableWidget(0, 5, c);
    m_pending->setHorizontalHeaderLabels({QStringLiteral("时间"), QStringLiteral("类型"),
                                          QStringLiteral("发送方"), QStringLiteral("金额"),
                                          QStringLiteral("交易 ID")});
    acs::styleTable(m_pending);
    inner->addWidget(m_pending, 1);

    auto *row = new QHBoxLayout();
    auto *rf = new QPushButton(QStringLiteral("刷新"), c);
    auto *acc = new QPushButton(QStringLiteral("接受收款"), c);
    acc->setObjectName(QStringLiteral("primary"));
    auto *rej = new QPushButton(QStringLiteral("拒收"), c);
    rej->setObjectName(QStringLiteral("danger"));
    row->addWidget(rf);
    row->addStretch(1);
    row->addWidget(acc);
    row->addWidget(rej);
    inner->addLayout(row);
    connect(rf, &QPushButton::clicked, this, &MainWindow::loadPending);
    connect(acc, &QPushButton::clicked, this, &MainWindow::confirmSelected);
    connect(rej, &QPushButton::clicked, this, &MainWindow::rejectSelected);

    v->addWidget(c);
    return page;
}

QWidget *MainWindow::pageLedger()
{
    auto *page = new QWidget(this);
    auto *v = new QVBoxLayout(page);
    v->setContentsMargins(0, 0, 0, 0);

    QVBoxLayout *inner = nullptr;
    auto *c = card(QStringLiteral("账本流水"), page, &inner);
    m_ledger = new QTableWidget(0, 5, c);
    m_ledger->setHorizontalHeaderLabels({QStringLiteral("时间"), QStringLiteral("类型"),
                                         QStringLiteral("对方"), QStringLiteral("金额"),
                                         QStringLiteral("状态")});
    acs::styleTable(m_ledger);
    inner->addWidget(m_ledger, 1);

    auto *row = new QHBoxLayout();
    auto *rf = new QPushButton(QStringLiteral("刷新流水"), c);
    row->addWidget(rf);
    row->addStretch(1);
    inner->addLayout(row);
    connect(rf, &QPushButton::clicked, this, &MainWindow::loadLedger);

    v->addWidget(c);
    return page;
}

QWidget *MainWindow::pageAbout()
{
    auto *page = new QWidget(this);
    auto *v = new QVBoxLayout(page);
    v->setContentsMargins(0, 0, 0, 0);

    QVBoxLayout *inner = nullptr;
    auto *c = card(QStringLiteral("关于"), page, &inner);
    auto *info = new QLabel(c);
    info->setWordWrap(true);
    info->setText(QStringLiteral("%1\n\n客户端版本：%2（后端 DLL %3，ABI %4）")
                      .arg(m_profile.appTitle,
                           QStringLiteral("3.3.0"),
                           Acs::version())
                      .arg(Acs::abiVersion()));
    inner->addWidget(info);
    v->addWidget(c);
    v->addStretch(1);
    return page;
}

void MainWindow::onLoggedIn()
{
    refreshState();
    doSync();
    loadPending();
    setPage(0);
}

void MainWindow::refreshState()
{
    QString err, code;
    QJsonObject st = Acs::call(QStringLiteral("state"), {}, &err, &code);
    if (!err.isEmpty()) {
        acs::showError(this, QStringLiteral("读取状态失败"), Acs::errorText(err, code));
        return;
    }
    const QString uid = st.value(QStringLiteral("uid")).toString();
    const QString type = st.value(QStringLiteral("type")).toString();
    const QString server = st.value(QStringLiteral("server_url")).toString();
    if (m_chip)
        m_chip->setText(QStringLiteral("%1 · %2").arg(uid, acs::typeLabel(type)));
    setBalanceAnimated(st.value(QStringLiteral("balance")).toVariant().toLongLong());
    m_status->setText(QStringLiteral("账户：%1（%2）\n状态：%3\n中心：%4\n上次同步：%5")
                          .arg(uid,
                               acs::typeLabel(type),
                               st.value(QStringLiteral("status")).toString(),
                               server,
                               acs::fmtTime(st.value(QStringLiteral("synced_at")).toVariant().toLongLong())));
    statusBar()->showMessage(QStringLiteral("状态已刷新"));
}

void MainWindow::doSync()
{
    statusBar()->showMessage(QStringLiteral("正在同步…"));
    QString err, code;
    QJsonObject r = Acs::call(QStringLiteral("sync"), {}, &err, &code);
    if (!err.isEmpty()) {
        statusBar()->showMessage(QStringLiteral("同步失败"));
        acs::showError(this, QStringLiteral("同步失败"), Acs::errorText(err, code));
        return;
    }
    statusBar()->showMessage(QStringLiteral("同步完成：交易 %1、账户快照 %2")
                                 .arg(r.value(QStringLiteral("txs")).toInt())
                                 .arg(r.value(QStringLiteral("accounts")).toInt()));
    refreshState();
}

void MainWindow::doTransfer()
{
    const QString to = m_toUid->text().trimmed();
    const QString pass = m_transferPass->text();
    bool ok = false;
    const qint64 amount = m_amount->text().trimmed().toLongLong(&ok);
    if (to.isEmpty() || !ok || amount <= 0 || pass.isEmpty()) {
        acs::showError(this, QStringLiteral("无法转账"),
                       QStringLiteral("请填写接收方、金额（正整数）与口令"));
        return;
    }
    QString err, code;
    QJsonObject data = Acs::call(QStringLiteral("transfer"),
                                 QJsonObject{
                                     {QStringLiteral("to"), to},
                                     {QStringLiteral("type"), m_toType->currentData().toString()},
                                     {QStringLiteral("amount"), amount},
                                     {QStringLiteral("password"), pass},
                                 },
                                 &err, &code);
    if (!err.isEmpty()) {
        acs::showError(this, QStringLiteral("转账失败"), Acs::errorText(err, code));
        return;
    }
    m_transferPass->clear();
    acs::showInfo(this, QStringLiteral("已提交"),
                  QStringLiteral("转账已提交到中心（状态：%1）。接收方接受后才入账。")
                      .arg(data.value(QStringLiteral("status")).toString()));
    refreshState();
}

void MainWindow::loadPending()
{
    QString err, code;
    QJsonObject data = Acs::call(QStringLiteral("pending"), {}, &err, &code);
    if (!err.isEmpty()) {
        statusBar()->showMessage(QStringLiteral("待收箱读取失败：%1").arg(Acs::errorText(err, code)));
        return;
    }
    const QJsonArray items = data.value(QStringLiteral("items")).toArray();
    m_pending->setRowCount(0);
    for (const auto &v : items) {
        const QJsonObject o = v.toObject();
        const int row = m_pending->rowCount();
        m_pending->insertRow(row);
        m_pending->setItem(row, 0, new QTableWidgetItem(
                                      acs::fmtTime(o.value(QStringLiteral("timestamp")).toVariant().toLongLong())));
        m_pending->setItem(row, 1, new QTableWidgetItem(o.value(QStringLiteral("tx_type")).toString()));
        m_pending->setItem(row, 2, new QTableWidgetItem(o.value(QStringLiteral("sender")).toString()));
        m_pending->setItem(row, 3, new QTableWidgetItem(
                                      acs::fmtAmount(o.value(QStringLiteral("amount")).toVariant().toLongLong())));
        m_pending->setItem(row, 4, new QTableWidgetItem(o.value(QStringLiteral("tx_id")).toString()));
    }
    statusBar()->showMessage(QStringLiteral("待收：%1 笔").arg(items.size()));
}

void MainWindow::loadLedger()
{
    QString err, code;
    QJsonObject data = Acs::call(QStringLiteral("ledger"), {}, &err, &code);
    if (!err.isEmpty()) {
        statusBar()->showMessage(QStringLiteral("流水读取失败：%1").arg(Acs::errorText(err, code)));
        return;
    }
    const QJsonArray txs = data.value(QStringLiteral("txs")).toArray();
    m_ledger->setRowCount(0);
    for (const auto &v : txs) {
        const QJsonArray a = v.toArray(); // [tx_id, tx_type, peer, peer_type, amount, ts, status, dir]
        if (a.size() < 8)
            continue;
        const int row = m_ledger->rowCount();
        m_ledger->insertRow(row);
        const qint64 amount = a.at(4).toVariant().toLongLong();
        const int dir = a.at(7).toInt();
        m_ledger->setItem(row, 0, new QTableWidgetItem(acs::fmtTime(a.at(5).toVariant().toLongLong())));
        m_ledger->setItem(row, 1, new QTableWidgetItem(a.at(1).toString()));
        m_ledger->setItem(row,
                          2,
                          new QTableWidgetItem(QStringLiteral("%1 · %2").arg(
                              a.at(2).toString(), acs::typeLabel(a.at(3).toString()))));
        auto *amt = new QTableWidgetItem(
            QStringLiteral("%1%2 A€").arg(dir >= 0 ? QStringLiteral("+") : QStringLiteral("-"),
                                          acs::fmtAmount(amount)));
        amt->setForeground(QColor(dir >= 0 ? QStringLiteral("#0A7D4B") : QStringLiteral("#E60028")));
        m_ledger->setItem(row, 3, amt);
        m_ledger->setItem(row, 4, new QTableWidgetItem(a.at(6).toString()));
    }
    statusBar()->showMessage(QStringLiteral("流水：%1 笔").arg(txs.size()));
}

QString MainWindow::selectedPendingTxId() const
{
    const int row = m_pending->currentRow();
    if (row < 0)
        return {};
    auto *it = m_pending->item(row, 4);
    return it ? it->text() : QString();
}

void MainWindow::confirmSelected()
{
    const QString txId = selectedPendingTxId();
    if (txId.isEmpty()) {
        acs::showError(this, QStringLiteral("无法接受"), QStringLiteral("请先在待收箱中选择一笔交易"));
        return;
    }
    bool ok = false;
    const QString pass = QInputDialog::getText(this, QStringLiteral("确认收款"),
                                               QStringLiteral("请输入钱包口令（用于签名）："),
                                               QLineEdit::Password, {}, &ok);
    if (!ok || pass.isEmpty())
        return;
    QString err, code;
    Acs::call(QStringLiteral("confirm"),
              QJsonObject{{QStringLiteral("tx_id"), txId}, {QStringLiteral("password"), pass}},
              &err, &code);
    if (!err.isEmpty()) {
        acs::showError(this, QStringLiteral("接受失败"), Acs::errorText(err, code));
        return;
    }
    acs::showInfo(this, QStringLiteral("已接受"), QStringLiteral("收款已确认，余额会在同步后更新。"));
    loadPending();
    doSync();
}

void MainWindow::rejectSelected()
{
    const QString txId = selectedPendingTxId();
    if (txId.isEmpty()) {
        acs::showError(this, QStringLiteral("无法拒收"), QStringLiteral("请先在待收箱中选择一笔交易"));
        return;
    }
    bool ok = false;
    const QString pass = QInputDialog::getText(this, QStringLiteral("拒收"),
                                               QStringLiteral("请输入钱包口令（用于签名）："),
                                               QLineEdit::Password, {}, &ok);
    if (!ok || pass.isEmpty())
        return;
    const QString reason = QInputDialog::getText(this, QStringLiteral("拒收理由"),
                                                 QStringLiteral("可填写拒收理由（可留空）："),
                                                 QLineEdit::Normal, {}, &ok);
    if (!ok)
        return;
    QString err, code;
    Acs::call(QStringLiteral("reject"),
              QJsonObject{{QStringLiteral("tx_id"), txId},
                          {QStringLiteral("password"), pass},
                          {QStringLiteral("reason"), reason}},
              &err, &code);
    if (!err.isEmpty()) {
        acs::showError(this, QStringLiteral("拒收失败"), Acs::errorText(err, code));
        return;
    }
    acs::showInfo(this, QStringLiteral("已拒收"), QStringLiteral("交易已拒收，金额将退回发送方。"));
    loadPending();
}

void MainWindow::doLogout()
{
    Acs::call(QStringLiteral("logout"));
    hide();
    qApp->exit(0);
}

void MainWindow::openPrivacy()
{
    QString err;
    QJsonObject brand = Acs::call(QStringLiteral("brand"), {}, &err);
    const QString url = brand.value(QStringLiteral("public_url")).toString() + QStringLiteral("/legal/privacy");
    if (!acs::openInBrowser(url))
        acs::showError(this, QStringLiteral("打开失败"), QStringLiteral("请手动访问：%1").arg(url));
}
