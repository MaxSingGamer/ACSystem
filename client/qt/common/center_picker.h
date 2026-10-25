// 启动页：选择中心服务（v3.3.0 #8/#11 / #15：历史按**服务名称**区分，无历史则「新添」）。
#pragma once

#include <QDialog>

class QListWidget;
class QLineEdit;
class QPushButton;
class QLabel;

/// 返回选中的中心地址（url）与是否用户取消。
class CenterPicker : public QDialog
{
    Q_OBJECT
public:
    explicit CenterPicker(const QString &appTitle, QWidget *parent = nullptr);

    /// 用户选中的中心地址（exec() == Accepted 时有效）。
    QString selectedUrl() const { return m_selectedUrl; }

protected:
    void accept() override;

private slots:
    void reload();
    void chooseCurrent();
    void addCenter();
    void removeCurrent();
    void onItemActivated();

private:
    QString labelOf(const QString &name, const QString &url, const QString &domain) const;

    QString m_appTitle;
    QString m_selectedUrl;
    QListWidget *m_list = nullptr;
    QLineEdit *m_urlEdit = nullptr;
    QLineEdit *m_nameEdit = nullptr;
    QPushButton *m_okBtn = nullptr;
    QLabel *m_hint = nullptr;
};
