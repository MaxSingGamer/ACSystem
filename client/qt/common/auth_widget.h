// 登录 / 注册页（v3.3.0 #6：必须选账户类型；国家/企业需全称；D30：类型白名单由客户端决定）。
#pragma once

#include <QWidget>

#include "ui_common.h"

class QComboBox;
class QLineEdit;
class QLabel;
class QCheckBox;
class QPushButton;

class AuthWidget : public QWidget
{
    Q_OBJECT
public:
    explicit AuthWidget(const acs::AppProfile &profile, QWidget *parent = nullptr);

    /// 中心地址变化时刷新（协议链接、标题）。
    void setCenter(const QString &serverUrl);

signals:
    void loggedIn();

private slots:
    void doLogin();
    void doRegister();
    void openTerms();
    void openPrivacy();

private:
    QString legalUrl(const QString &doc) const;

    acs::AppProfile m_profile;
    QString m_serverUrl;
    QComboBox *m_type = nullptr;
    QLineEdit *m_uid = nullptr;
    QLineEdit *m_pass = nullptr;
    QCheckBox *m_agree = nullptr;
    QLabel *m_hint = nullptr;
    QPushButton *m_loginBtn = nullptr;
};
