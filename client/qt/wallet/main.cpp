// AlphaWallet（个人 + 国家 + 系统账户）——QT 客户端入口（D30：与 AlphaBankmanage 互相排斥）。
//
// 启动流程（v3.3.0 #8/#11）：选中心服务（历史按服务名称区分 #15）→ 登录/注册 → 主窗口。
//
// 开发/自动化参数（仅用于截图与自动化测试，正常用户无需使用）：
//   --center <url>       跳过启动页，直接使用该中心
//   --dev-login u,p      跳过登录页，直接用该账户登录（口令在命令行可见，仅限本机测试）
#ifdef _WIN32
#  include <windows.h>
namespace {
// 高分屏：声明 Per-Monitor V2 DPI 感知（必须在 QApplication 创建前调用）。
// 否则 Windows 会对窗口做位图拉伸 → 文字发虚、窗口/截图坐标错位。
void enableDpiAwareness()
{
    if (auto *user32 = LoadLibraryW(L"user32.dll")) {
        using SetCtxFn = BOOL(WINAPI *)(HANDLE);
        auto fn = reinterpret_cast<SetCtxFn>(GetProcAddress(user32, "SetProcessDpiAwarenessContext"));
        if (fn)
            fn(reinterpret_cast<HANDLE>(-4)); // DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2
    }
}
} // namespace
#endif
#include <QApplication>
#include <QDialog>
#include <QIcon>
#include <QStringList>

#include "acs_abi.h"
#include "auth_widget.h"
#include "center_picker.h"
#include "main_window.h"
#include "ui_common.h"

namespace {
QString argValue(const QStringList &args, const QString &key)
{
    for (int i = 0; i < args.size(); ++i) {
        if (args.at(i) == key && i + 1 < args.size())
            return args.at(i + 1);
        if (args.at(i).startsWith(key + QLatin1Char('=')))
            return args.at(i).mid(key.size() + 1);
    }
    return {};
}
} // namespace

int main(int argc, char *argv[])
{
#ifdef _WIN32
    enableDpiAwareness();
#endif
    QApplication app(argc, argv);
    app.setWindowIcon(QIcon(QStringLiteral(":/icon-client.ico")));
    acs::applyTheme(app);
    app.setApplicationName(QStringLiteral("AlphaWallet"));

    const QString initErr = Acs::init();
    if (!initErr.isEmpty()) {
        acs::showError(nullptr, QStringLiteral("启动失败"), initErr);
        return 1;
    }

    // 个人版：只允许 个人 / 国家 / 系统（企业账户请用 AlphaBankmanage）
    acs::AppProfile profile{
        QStringLiteral("Alpha Wallet"),
        QStringLiteral("Alpha Wallet"),
        {QStringLiteral("Individual"), QStringLiteral("Country"), QStringLiteral("System")},
        true,
    };

    const QStringList args = app.arguments();
    QString center = argValue(args, QStringLiteral("--center"));
    const QString devLogin = argValue(args, QStringLiteral("--dev-login"));

    if (center.isEmpty()) {
        CenterPicker picker(profile.appTitle);
        if (picker.exec() != QDialog::Accepted)
            return 0;
        center = picker.selectedUrl();
    } else {
        QString e, code;
        Acs::call(QStringLiteral("set_server"), {{QStringLiteral("url"), center}}, &e, &code);
        if (!e.isEmpty()) {
            acs::showError(nullptr, QStringLiteral("连接中心失败"), Acs::errorText(e, code));
            return 1;
        }
    }

    AuthWidget auth(profile);
    auth.setCenter(center);
    auth.setWindowTitle(profile.appTitle);
    auth.resize(560, 540);

    MainWindow mainWin(profile);

    if (!devLogin.isEmpty()) {
        const QStringList parts = devLogin.split(QLatin1Char(','));
        if (parts.size() >= 2) {
            QString e, code;
            Acs::call(QStringLiteral("login"),
                      {{QStringLiteral("uid"), parts.at(0)},
                       {QStringLiteral("password"), parts.at(1)},
                       {QStringLiteral("agree_terms"), true},
                       {QStringLiteral("agree_privacy"), true}},
                      &e, &code);
            if (!e.isEmpty()) {
                acs::showError(nullptr, QStringLiteral("登录失败"), Acs::errorText(e, code));
                return 1;
            }
            mainWin.onLoggedIn();
            mainWin.show();
            return app.exec();
        }
    }

    QObject::connect(&auth, &AuthWidget::loggedIn, &mainWin, [&] {
        mainWin.onLoggedIn();
        mainWin.show();
        auth.hide();
    });
    auth.show();
    return app.exec();
}