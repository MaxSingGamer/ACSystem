#include "center_picker.h"

#include "acs_abi.h"
#include "ui_common.h"

#include <QFormLayout>
#include <QHBoxLayout>
#include <QJsonArray>
#include <QJsonObject>
#include <QLabel>
#include <QLineEdit>
#include <QListWidget>
#include <QPushButton>
#include <QVBoxLayout>

CenterPicker::CenterPicker(const QString &appTitle, QWidget *parent)
    : QDialog(parent)
    , m_appTitle(appTitle)
{
    setWindowTitle(QStringLiteral("%1 · 选择中心服务").arg(appTitle));
    setMinimumWidth(640);
    resize(660, 620);
    setStyleSheet(acs::darkStyleSheet());

    auto *root = new QVBoxLayout(this);
    auto *title = new QLabel(QStringLiteral("选择中心服务"), this);
    title->setObjectName(QStringLiteral("title"));
    root->addWidget(title);
    auto *hint = new QLabel(
        QStringLiteral("每次启动都需要选择中心服务。历史记录按「服务名称」区分；没有您的中心就点「新添」。"),
        this);
    hint->setObjectName(QStringLiteral("muted"));
    hint->setWordWrap(true);
    root->addWidget(hint);

    m_list = new QListWidget(this);
    m_list->setMinimumHeight(180);
    root->addWidget(m_list, 1);

    auto *form = new QFormLayout();
    m_urlEdit = new QLineEdit(this);
    m_urlEdit->setPlaceholderText(QStringLiteral("如 https://acsystem.maxshin.top 或 http://127.0.0.1:9600"));
    m_nameEdit = new QLineEdit(this);
    m_nameEdit->setPlaceholderText(QStringLiteral("可选：服务名称（用于历史列表显示）"));
    form->addRow(QStringLiteral("中心地址"), m_urlEdit);
    form->addRow(QStringLiteral("备注名称"), m_nameEdit);
    root->addLayout(form);

    auto *row = new QHBoxLayout();
    auto *addBtn = new QPushButton(QStringLiteral("新添并进入"), this);
    addBtn->setObjectName(QStringLiteral("primary"));
    auto *delBtn = new QPushButton(QStringLiteral("删除所选历史"), this);
    delBtn->setObjectName(QStringLiteral("danger"));
    m_okBtn = new QPushButton(QStringLiteral("进入所选中心"), this);
    auto *cancelBtn = new QPushButton(QStringLiteral("退出"), this);
    row->addWidget(addBtn);
    row->addWidget(delBtn);
    row->addStretch(1);
    row->addWidget(m_okBtn);
    row->addWidget(cancelBtn);
    root->addLayout(row);

    m_hint = new QLabel(this);
    m_hint->setObjectName(QStringLiteral("muted"));
    m_hint->setWordWrap(true);
    root->addWidget(m_hint);

    connect(addBtn, &QPushButton::clicked, this, &CenterPicker::addCenter);
    connect(delBtn, &QPushButton::clicked, this, &CenterPicker::removeCurrent);
    connect(m_okBtn, &QPushButton::clicked, this, &CenterPicker::chooseCurrent);
    connect(cancelBtn, &QPushButton::clicked, this, &QDialog::reject);
    connect(m_list, &QListWidget::itemActivated, this, &CenterPicker::onItemActivated);
    connect(m_list, &QListWidget::itemSelectionChanged, this, [this] {
        auto *it = m_list->currentItem();
        if (!it)
            return;
        m_urlEdit->setText(it->data(Qt::UserRole).toString());
    });

    reload();
}

QString CenterPicker::labelOf(const QString &name, const QString &url, const QString &domain) const
{
    const QString shown = name.trimmed().isEmpty() ? domain : name.trimmed();
    return QStringLiteral("%1\n%2").arg(shown, url);
}

void CenterPicker::reload()
{
    m_list->clear();
    QString err;
    QJsonObject data = Acs::call(QStringLiteral("centers"), {}, &err);
    if (!err.isEmpty() && err != QStringLiteral("internal")) {
        // centers 是纯本地读取，失败通常意味着目录权限问题
        m_hint->setText(QStringLiteral("读取中心历史失败：%1").arg(err));
    }
    const QJsonArray items = data.value(QStringLiteral("items")).toArray();
    for (const auto &v : items) {
        const QJsonObject o = v.toObject();
        const QString domain = o.value(QStringLiteral("domain")).toString();
        const QString url = o.value(QStringLiteral("url")).toString();
        const QString name = o.value(QStringLiteral("name")).toString();
        auto *it = new QListWidgetItem(labelOf(name, url, domain), m_list);
        it->setData(Qt::UserRole, url);
        it->setData(Qt::UserRole + 1, domain);
        if (o.value(QStringLiteral("current")).toBool()) {
            it->setText(it->text() + QStringLiteral("\n（上次使用）"));
        }
    }
    if (m_list->count() > 0) {
        m_list->setCurrentRow(0);
    } else {
        m_hint->setText(QStringLiteral("还没有历史中心：请在下方填写中心地址后点「新添并进入」。"));
    }
}

void CenterPicker::onItemActivated()
{
    chooseCurrent();
}

void CenterPicker::chooseCurrent()
{
    auto *it = m_list->currentItem();
    if (!it) {
        m_hint->setText(QStringLiteral("请先在列表中选择一个历史中心，或在下方「新添」。"));
        return;
    }
    QString url = it->data(Qt::UserRole).toString();
    if (url.trimmed().isEmpty()) {
        m_hint->setText(QStringLiteral("该历史记录缺少地址，请删除后重新添加。"));
        return;
    }
    m_selectedUrl = url;
    accept();
}

void CenterPicker::addCenter()
{
    QString url = m_urlEdit->text().trimmed();
    if (url.isEmpty()) {
        m_hint->setText(QStringLiteral("请填写中心地址（如 https://acsystem.maxshin.top）"));
        return;
    }
    if (!url.startsWith(QStringLiteral("http://")) && !url.startsWith(QStringLiteral("https://")))
        url.prepend(QStringLiteral("http://"));
    QString err, code;
    // set_server 会写入历史并拉取该中心的客户端配置（含 service_name，供列表展示）
    QJsonObject data = Acs::call(QStringLiteral("set_server"),
                                 QJsonObject{{QStringLiteral("url"), url}}, &err, &code);
    if (!err.isEmpty()) {
        m_hint->setText(QStringLiteral("连接中心失败：%1\n（请确认地址正确、服务已启动）")
                            .arg(Acs::errorText(err, code)));
        return;
    }
    m_selectedUrl = data.value(QStringLiteral("server_url")).toString();
    if (m_selectedUrl.isEmpty())
        m_selectedUrl = url;
    accept();
}

void CenterPicker::removeCurrent()
{
    auto *it = m_list->currentItem();
    if (!it)
        return;
    const QString domain = it->data(Qt::UserRole + 1).toString();
    if (domain.isEmpty())
        return;
    QString err;
    Acs::call(QStringLiteral("center_remove"),
              QJsonObject{{QStringLiteral("domain"), domain}}, &err);
    if (!err.isEmpty()) {
        m_hint->setText(QStringLiteral("删除失败：%1").arg(err));
        return;
    }
    reload();
}

void CenterPicker::accept()
{
    if (m_selectedUrl.isEmpty()) {
        m_hint->setText(QStringLiteral("请选择或新添一个中心服务"));
        return;
    }
    QDialog::accept();
}
