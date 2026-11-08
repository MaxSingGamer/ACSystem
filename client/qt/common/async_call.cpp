// 异步 ABI 调用：把阻塞的 Acs::call（网络/磁盘）放到后台线程执行，
// 期间在所属窗口显示加载遮罩（旋转指示器动画），完成后回 UI 线程回调。
#include "async_call.h"

#include "acs_abi.h"

#include <QCoreApplication>
#include <QEvent>
#include <QFrame>
#include <QGraphicsDropShadowEffect>
#include <QHBoxLayout>
#include <QKeyEvent>
#include <QLabel>
#include <QMutex>
#include <QMutexLocker>
#include <QMouseEvent>
#include <QPaintEvent>
#include <QPainter>
#include <QResizeEvent>
#include <QPointer>
#include <QTimer>
#include <QVBoxLayout>
#include <QWheelEvent>
#include <QWidget>

#include <thread>

namespace {

/// 品牌蓝旋转圆环（轻量自绘，不依赖系统进度条样式）。
class Spinner : public QWidget
{
public:
    explicit Spinner(QWidget *parent)
        : QWidget(parent)
    {
        setFixedSize(38, 38);
        m_timer.setInterval(28);
        QObject::connect(&m_timer, &QTimer::timeout, this, [this] {
            m_angle = (m_angle + 24) % 360;
            update();
        });
        m_timer.start();
    }

protected:
    void paintEvent(QPaintEvent *) override
    {
        QPainter p(this);
        p.setRenderHint(QPainter::Antialiasing, true);
        const QRectF r = QRectF(rect()).adjusted(4.0, 4.0, -4.0, -4.0);
        QPen track(QColor(5, 109, 174, 45));
        track.setWidthF(3.6);
        track.setCapStyle(Qt::RoundCap);
        p.setPen(track);
        p.drawEllipse(r.adjusted(1.8, 1.8, -1.8, -1.8));
        QPen arc(QColor(QStringLiteral("#056DAE")));
        arc.setWidthF(3.6);
        arc.setCapStyle(Qt::RoundCap);
        p.setPen(arc);
        p.drawArc(r, (270 - m_angle) * 16, 96 * 16);
    }

private:
    QTimer m_timer;
    int m_angle = 0;
};

/// 加载遮罩：半透明海军蓝背景 + 白色圆角卡片（旋转环 + 文案）。
class BusyOverlay : public QWidget
{
public:
    explicit BusyOverlay(QWidget *window)
        : QWidget(window)
    {
        setObjectName(QStringLiteral("acsBusyOverlay"));
        setAttribute(Qt::WA_StyledBackground, true);
        setStyleSheet(QStringLiteral("QWidget#acsBusyOverlay{background:rgba(11,37,69,96);}"));

        auto *lay = new QVBoxLayout(this);
        lay->setContentsMargins(0, 0, 0, 0);
        lay->addStretch(1);
        auto *h = new QHBoxLayout();
        h->addStretch(1);

        auto *card = new QFrame(this);
        card->setObjectName(QStringLiteral("acsBusyCard"));
        card->setStyleSheet(
            QStringLiteral("QFrame#acsBusyCard{background:#FFFFFF;border-radius:16px;}"));
        card->setFixedSize(300, 158);
        auto *shadow = new QGraphicsDropShadowEffect(card);
        shadow->setBlurRadius(34);
        shadow->setOffset(0, 10);
        shadow->setColor(QColor(0, 0, 0, 70));
        card->setGraphicsEffect(shadow);

        auto *v = new QVBoxLayout(card);
        v->setContentsMargins(26, 22, 26, 20);
        v->setSpacing(10);
        auto *spinner = new Spinner(card);
        auto *spinnerRow = new QHBoxLayout();
        spinnerRow->addStretch(1);
        spinnerRow->addWidget(spinner);
        spinnerRow->addStretch(1);
        v->addLayout(spinnerRow);
        m_text = new QLabel(QStringLiteral("正在从中心加载…"), card);
        m_text->setAlignment(Qt::AlignCenter);
        m_text->setWordWrap(true);
        m_text->setStyleSheet(QStringLiteral(
            "color:#0B2545;font-weight:600;font-size:13.5px;"));
        auto *sub = new QLabel(QStringLiteral("网络较慢时请稍候…"), card);
        sub->setAlignment(Qt::AlignCenter);
        sub->setStyleSheet(QStringLiteral("color:#8A97AB;font-size:11.5px;"));
        v->addWidget(m_text);
        v->addWidget(sub);

        h->addWidget(card);
        h->addStretch(1);
        lay->addLayout(h);
        lay->addStretch(1);

        hide();
        if (window) {
            window->installEventFilter(this);
            setGeometry(window->rect());
        }
        setFocusPolicy(Qt::StrongFocus);
    }

    void push(const QString &text)
    {
        if (!text.isEmpty())
            m_text->setText(text);
        m_count += 1;
        if (parentWidget())
            setGeometry(parentWidget()->rect());
        raise();
        show();
        setFocus();
    }

    void pop()
    {
        m_count -= 1;
        if (m_count <= 0) {
            m_count = 0;
            hide();
        }
    }

protected:
    bool eventFilter(QObject *obj, QEvent *ev) override
    {
        if ((ev->type() == QEvent::Resize || ev->type() == QEvent::Show) && obj == parentWidget()) {
            setGeometry(parentWidget()->rect());
            raise();
        }
        return QWidget::eventFilter(obj, ev);
    }

    void mousePressEvent(QMouseEvent *e) override { e->accept(); }
    void mouseReleaseEvent(QMouseEvent *e) override { e->accept(); }
    void mouseDoubleClickEvent(QMouseEvent *e) override { e->accept(); }
    void wheelEvent(QWheelEvent *e) override { e->accept(); }
    void keyPressEvent(QKeyEvent *e) override { e->accept(); }
    void resizeEvent(QResizeEvent *) override
    {
        if (parentWidget())
            setGeometry(parentWidget()->rect());
    }

private:
    QLabel *m_text = nullptr;
    int m_count = 0;
};

BusyOverlay *ensureOverlay(QWidget *context)
{
    if (!context)
        return nullptr;
    QWidget *win = context->window();
    if (!win)
        return nullptr;
    auto *existing = win->findChild<QWidget *>(QStringLiteral("acsBusyOverlay"));
    if (existing)
        return static_cast<BusyOverlay *>(existing);
    return new BusyOverlay(win);
}

} // namespace

namespace acs {

void runAsync(const QString &method,
              const QJsonObject &req,
              QWidget *context,
              const QString &busyText,
              AsyncCallback cb)
{
    QPointer<QWidget> guard(context);
    BusyOverlay *overlay = ensureOverlay(context);
    if (overlay)
        overlay->push(busyText);

    std::thread([method, req, guard, busyText, cb]() {
        QJsonObject data;
        QString err;
        QString code;
        // 串行化由 Acs::call 内部统一负责（UI 线程的同步调用也走同一把锁）。
        data = Acs::call(method, req, &err, &code);
        QCoreApplication *app = QCoreApplication::instance();
        if (!app)
            return;
        QMetaObject::invokeMethod(
            app,
            [guard, busyText, cb, data, err, code]() {
                if (!guard)
                    return;
                if (auto *w = guard.data()->window()) {
                    auto *ov = w->findChild<QWidget *>(QStringLiteral("acsBusyOverlay"));
                    if (auto *busy = static_cast<BusyOverlay *>(ov))
                        busy->pop();
                }
                if (cb)
                    cb(data, err, code);
            },
            Qt::QueuedConnection);
    }).detach();
}

} // namespace acs

