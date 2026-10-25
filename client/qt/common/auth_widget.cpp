#include "auth_widget.h"

#include "acs_abi.h"

#include <QCheckBox>
#include <QComboBox>
#include <QDialog>
#include <QDialogButtonBox>
#include <QFormLayout>
#include <QHBoxLayout>
#include <QJsonObject>
#include <QLabel>
#include <QLineEdit>
#include <QPushButton>
#include <QUrl>
#include <QVBoxLayout>

namespace {

/// 注册对话框：成功后返回 true。
bool runRegisterDialog(QWidget *parent, const acs::AppProfile &profile, const QString &serverUrl)
{
    QDialog dlg(parent);
    dlg.setWindowTitle(QStringLiteral("注册新账户 · %1").arg(profile.appTitle));
    dlg.setStyleSheet(acs::darkStyleSheet());
    dlg.setMinimumWidth(460);

    auto *root = new QVBoxLayout(&dlg);
    auto *tip = new QLabel(QStringLiteral("注册即表示同意中心的《使用协议》与《隐私政策》（可在登录页打开阅读）。"), &dlg);
    tip->setObjectName(QStringLiteral("muted"));
    tip->setWordWrap(true);
    root->addWidget(tip);

    auto *form = new QFormLayout();
    auto *type = new QComboBox(&dlg);
    acs::fillTypeCombo(type, profile.types);
    auto *fullname = new QLineEdit(&dlg);
    auto *uid = new QLineEdit(&dlg);
    auto *email = new QLineEdit(&dlg);
    auto *pass = new QLineEdit(&dlg);
    pass->setEchoMode(QLineEdit::Password);
    email->setPlaceholderText(QStringLiteral("如 name@example.com"));
    fullname->setPlaceholderText(QStringLiteral("国家/企业全称（同类型内唯一）"));
    form->addRow(QStringLiteral("账户类型"), type);
    form->addRow(QStringLiteral("全称"), fullname);
    form->addRow(QStringLiteral("简称 / UID"), uid);
    form->addRow(QStringLiteral("邮箱"), email);
    form->addRow(QStringLiteral("口令（≥8 位）"), pass);
    root->addLayout(form);

    auto syncFields = [&] {
        const QString t = type->currentData().toString();
        const bool needFull = (t == QStringLiteral("Country") || t == QStringLiteral("Company"));
        fullname->setEnabled(needFull);
        if (t == QStringLiteral("Country")) {
            uid->setMaxLength(2);
            uid->setPlaceholderText(QStringLiteral("2 位大写字母，如 LE"));
        } else if (t == QStringLiteral("Company")) {
            uid->setMaxLength(4);
            uid->setPlaceholderText(QStringLiteral("4 位大写字母，如 ALPH"));
        } else {
            uid->setMaxLength(64);
            uid->setPlaceholderText(QStringLiteral("个人 UID，如 Steve"));
        }
    };
    QObject::connect(type, &QComboBox::currentIndexChanged, &dlg, syncFields);
    syncFields();

    auto *err = new QLabel(&dlg);
    err->setStyleSheet(QStringLiteral("color:#ef5350;"));
    err->setWordWrap(true);
    root->addWidget(err);

    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel, &dlg);
    buttons->button(QDialogButtonBox::Ok)->setText(QStringLiteral("注册"));
    buttons->button(QDialogButtonBox::Cancel)->setText(QStringLiteral("取消"));
    root->addWidget(buttons);
    QObject::connect(buttons, &QDialogButtonBox::rejected, &dlg, &QDialog::reject);

    QObject::connect(buttons, &QDialogButtonBox::accepted, &dlg, [&] {
        const QString t = type->currentData().toString();
        const QString u = uid->text().trimmed();
        if (u.isEmpty() || email->text().trimmed().isEmpty() || pass->text().size() < 8) {
            err->setText(QStringLiteral("请填写完整：简称、邮箱、口令（≥8 位）"));
            return;
        }
        const bool needFull = (t == QStringLiteral("Country") || t == QStringLiteral("Company"));
        if (needFull && fullname->text().trimmed().isEmpty()) {
            err->setText(QStringLiteral("国家/企业账户必须填写全称"));
            return;
        }
        QString e, code;
        Acs::call(QStringLiteral("register"),
                  QJsonObject{
                      {QStringLiteral("server_url"), serverUrl},
                      {QStringLiteral("uid"), u},
                      {QStringLiteral("type"), t},
                      {QStringLiteral("fullname"), fullname->text().trimmed()},
                      {QStringLiteral("email"), email->text().trimmed()},
                      {QStringLiteral("password"), pass->text()},
                      {QStringLiteral("agree_terms"), true},
                      {QStringLiteral("agree_privacy"), true},
                  },
                  &e, &code);
        if (!e.isEmpty()) {
            err->setText(Acs::errorText(e, code));
            return;
        }
        acs::showInfo(&dlg, QStringLiteral("注册成功"),
                      QStringLiteral("账户已注册。请用刚填写的口令登录。"));
        dlg.accept();
    });

    return dlg.exec() == QDialog::Accepted;
}

} // namespace

AuthWidget::AuthWidget(const acs::AppProfile &profile, QWidget *parent)
    : QWidget(parent)
    , m_profile(profile)
{
    setStyleSheet(acs::darkStyleSheet());
    auto *root = new QVBoxLayout(this);
    root->setContentsMargins(40, 32, 40, 32);
    root->setSpacing(14);

    auto *title = new QLabel(QStringLiteral("%1 · 登录").arg(profile.appTitle), this);
    title->setObjectName(QStringLiteral("title"));
    root->addWidget(title);

    m_hint = new QLabel(this);
    m_hint->setObjectName(QStringLiteral("muted"));
    m_hint->setWordWrap(true);
    root->addWidget(m_hint);

    auto *form = new QFormLayout();
    m_type = new QComboBox(this);
    acs::fillTypeCombo(m_type, profile.types);
    m_uid = new QLineEdit(this);
    m_uid->setPlaceholderText(QStringLiteral("账户 UID / 简称"));
    m_pass = new QLineEdit(this);
    m_pass->setEchoMode(QLineEdit::Password);
    m_pass->setPlaceholderText(QStringLiteral("钱包口令（≥8 位）"));
    form->addRow(QStringLiteral("账户类型"), m_type);
    form->addRow(QStringLiteral("账户"), m_uid);
    form->addRow(QStringLiteral("口令"), m_pass);
    root->addLayout(form);

    auto *links = new QHBoxLayout();
    m_agree = new QCheckBox(QStringLiteral("我已阅读并同意"), this);
    auto *terms = new QPushButton(QStringLiteral("《使用协议》"), this);
    terms->setFlat(true);
    auto *privacy = new QPushButton(QStringLiteral("《隐私政策》"), this);
    privacy->setFlat(true);
    links->addWidget(m_agree);
    links->addWidget(terms);
    links->addWidget(privacy);
    links->addStretch(1);
    root->addLayout(links);

    auto *btns = new QHBoxLayout();
    m_loginBtn = new QPushButton(QStringLiteral("登录"), this);
    m_loginBtn->setObjectName(QStringLiteral("primary"));
    auto *reg = new QPushButton(QStringLiteral("注册新账户"), this);
    btns->addWidget(m_loginBtn);
    btns->addWidget(reg);
    btns->addStretch(1);
    root->addLayout(btns);
    root->addStretch(1);

    connect(terms, &QPushButton::clicked, this, &AuthWidget::openTerms);
    connect(privacy, &QPushButton::clicked, this, &AuthWidget::openPrivacy);
    connect(m_loginBtn, &QPushButton::clicked, this, &AuthWidget::doLogin);
    connect(reg, &QPushButton::clicked, this, &AuthWidget::doRegister);
    connect(m_pass, &QLineEdit::returnPressed, this, &AuthWidget::doLogin);
}

void AuthWidget::setCenter(const QString &serverUrl)
{
    m_serverUrl = serverUrl;
    m_hint->setText(QStringLiteral("中心：%1\n请选择账户类型并输入账户与口令。").arg(serverUrl));
}

QString AuthWidget::legalUrl(const QString &doc) const
{
    QString base = m_serverUrl.trimmed();
    while (base.endsWith(QLatin1Char('/')))
        base.chop(1);
    return QStringLiteral("%1/legal/%2").arg(base, doc);
}

void AuthWidget::openTerms()
{
    const QString t = m_type->currentData().toString();
    const QString doc = (t == QStringLiteral("Individual") || t == QStringLiteral("System"))
                            ? QStringLiteral("individual-terms")
                            : QStringLiteral("enterprise-terms");
    if (!acs::openInBrowser(legalUrl(doc))) {
        acs::showError(this, QStringLiteral("打开失败"),
                       QStringLiteral("无法打开浏览器，请手动访问：\n%1").arg(legalUrl(doc)));
    }
}

void AuthWidget::openPrivacy()
{
    if (!acs::openInBrowser(legalUrl(QStringLiteral("privacy")))) {
        acs::showError(this, QStringLiteral("打开失败"),
                       QStringLiteral("无法打开浏览器，请手动访问：\n%1").arg(legalUrl(QStringLiteral("privacy"))));
    }
}

void AuthWidget::doLogin()
{
    if (m_uid->text().trimmed().isEmpty() || m_pass->text().isEmpty()) {
        acs::showError(this, QStringLiteral("无法登录"), QStringLiteral("请填写账户与口令"));
        return;
    }
    if (!m_agree->isChecked()) {
        acs::showError(this, QStringLiteral("无法登录"),
                       QStringLiteral("请先勾选同意《使用协议》与《隐私政策》"));
        return;
    }
    m_loginBtn->setEnabled(false);
    QString err, code;
    QJsonObject data = Acs::call(QStringLiteral("login"),
                                 QJsonObject{
                                     {QStringLiteral("uid"), m_uid->text().trimmed()},
                                     {QStringLiteral("password"), m_pass->text()},
                                     {QStringLiteral("agree_terms"), true},
                                     {QStringLiteral("agree_privacy"), true},
                                 },
                                 &err, &code);
    m_loginBtn->setEnabled(true);
    if (!err.isEmpty()) {
        acs::showError(this, QStringLiteral("登录失败"), Acs::errorText(err, code));
        return;
    }
    m_pass->clear();
    emit loggedIn();
}

void AuthWidget::doRegister()
{
    if (!runRegisterDialog(this, m_profile, m_serverUrl))
        return;
    // 注册成功：把 UID 填进登录框方便直接登录
}
