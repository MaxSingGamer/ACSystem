// ABI 冒烟测试（控制台）：验证 Qt 工程能加载 acs_client_core.dll 并正确调用 C ABI。
// 这是 QT 客户端的"第一道门"：先跑通它，再写界面。
//
// 用法：abi_smoke.exe [--dll <path>]
// 退出码：0 全通过；非 0 = 失败项数。
#include <QCoreApplication>
#include <QJsonObject>
#include <QTextStream>

#include "acs_abi.h"

static int g_ok = 0;
static int g_fail = 0;

static QTextStream &out()
{
    static QTextStream s(stdout);
    return s;
}

static void check(const QString &name, bool pass, const QString &detail = {})
{
    if (pass) {
        ++g_ok;
    } else {
        ++g_fail;
    }
    out() << (pass ? "[PASS] " : "[FAIL] ") << name;
    if (!detail.isEmpty()) {
        out() << "  -> " << detail;
    }
    out() << "\n";
    out().flush();
}

int main(int argc, char *argv[])
{
    QCoreApplication app(argc, argv);

    out() << "=== 1. DLL 加载与 ABI 版本 ===\n";
    const QString initErr = Acs::init();
    check(QStringLiteral("加载 acs_client_core.dll"), initErr.isEmpty(), initErr);
    if (!initErr.isEmpty()) {
        out() << "\n总计 " << (g_ok + g_fail) << " 项，失败 " << g_fail << " 项\n";
        return g_fail;
    }
    check(QStringLiteral("ABI 版本匹配"), Acs::abiVersion() == Acs::ABI_EXPECTED,
          QStringLiteral("expected=%1 got=%2").arg(Acs::ABI_EXPECTED).arg(Acs::abiVersion()));
    check(QStringLiteral("产品版本可读"), !Acs::version().isEmpty(), Acs::version());

    out() << "=== 2. info 方法 ===\n";
    QString err, code;
    QJsonObject info = Acs::call(QStringLiteral("info"), {}, &err, &code);
    check(QStringLiteral("info.ok"), err.isEmpty(), Acs::errorText(err, code));
    check(QStringLiteral("info.data.abi_version"),
          info.value(QStringLiteral("abi_version")).toInt() == static_cast<int>(Acs::ABI_EXPECTED),
          QString::number(info.value(QStringLiteral("abi_version")).toInt()));
    check(QStringLiteral("info.data.version 与 acs_version 一致"),
          info.value(QStringLiteral("version")).toString() == Acs::version(),
          info.value(QStringLiteral("version")).toString());

    out() << "=== 3. 错误路径（不 panic / 结构化） ===\n";
    err.clear();
    code.clear();
    Acs::call(QStringLiteral("no_such_method"), {}, &err, &code);
    check(QStringLiteral("未知方法 -> unknown_method"), code == QStringLiteral("unknown_method"),
          Acs::errorText(err, code));

    out() << "=== 4. 纯本地业务方法 legal_doc ===\n";
    err.clear();
    code.clear();
    QJsonObject doc = Acs::call(QStringLiteral("legal_doc"),
                                QJsonObject{{QStringLiteral("doc"), QStringLiteral("privacy")}},
                                &err, &code);
    const QString html = doc.value(QStringLiteral("html")).toString();
    check(QStringLiteral("legal_doc 返回 HTML"), err.isEmpty() && html.contains(QStringLiteral("<html")),
          QStringLiteral("len=%1").arg(html.size()));

    out() << "=== 5. brand（无中心时中性默认） ===\n";
    err.clear();
    code.clear();
    QJsonObject brand = Acs::call(QStringLiteral("brand"), {}, &err, &code);
    check(QStringLiteral("brand 返回并标识来源"),
          err.isEmpty() && !brand.value(QStringLiteral("source")).toString().isEmpty(),
          QStringLiteral("source=%1 name=%2")
              .arg(brand.value(QStringLiteral("source")).toString(),
                   brand.value(QStringLiteral("name")).toString()));
    check(QStringLiteral("brand 不回显口令字段"),
          !brand.contains(QStringLiteral("password")) && !brand.contains(QStringLiteral("seckey")), {});

    out() << "\n总计 " << (g_ok + g_fail) << " 项，失败 " << g_fail << " 项\n";
    return g_fail;
}
