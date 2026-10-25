#include "acs_abi.h"

#include <QJsonDocument>
#include <QLibrary>
#include <QDebug>

#include <memory>

#if defined(Q_OS_WIN)
#  include <windows.h>
#endif

namespace {

using AbiVersionFn = unsigned (*)();
using VersionFn = const char *(*)();
using CallFn = char *(*)(const char *method, const char *req);
using FreeFn = void (*)(char *p);

struct Bindings {
    AbiVersionFn abiVersion = nullptr;
    VersionFn version = nullptr;
    CallFn call = nullptr;
    FreeFn freeString = nullptr;
    bool ok = false;
    QString error;
};

Bindings &bindings()
{
    static Bindings b;
    static bool tried = false;
    if (tried) {
        return b;
    }
    tried = true;

#if defined(Q_OS_WIN)
    // 优先按 CMake 传入的绝对路径加载（开发期指向 target/debug|release）；
    // 打包后 DLL 与 exe 同目录，Windows 会用应用目录搜索，这里留空回退名字即可。
    QString path;
#  if defined(ACS_CLIENT_CORE_DLL_PATH)
    path = QStringLiteral(ACS_CLIENT_CORE_DLL_PATH);
#  endif
    auto lib = std::make_unique<QLibrary>(path.isEmpty() ? QStringLiteral("acs_client_core") : path);
    if (!lib->load()) {
        b.error = QStringLiteral("无法加载 acs_client_core.dll：%1（请确认 DLL 与程序同目录）")
                      .arg(lib->errorString());
        return b;
    }
    b.abiVersion = reinterpret_cast<AbiVersionFn>(lib->resolve("acs_abi_version"));
    b.version = reinterpret_cast<VersionFn>(lib->resolve("acs_version"));
    b.call = reinterpret_cast<CallFn>(lib->resolve("acs_call"));
    b.freeString = reinterpret_cast<FreeFn>(lib->resolve("acs_string_free"));
    if (!b.abiVersion || !b.version || !b.call || !b.freeString) {
        b.error = QStringLiteral("acs_client_core.dll 缺少必需导出符号（ABI 不兼容或文件损坏）");
        return b;
    }
    // 句柄常驻：QLibrary 析构会卸载 DLL，这里故意泄漏到进程结束
    lib.release();
    b.ok = true;
    return b;
#else
    b.error = QStringLiteral("当前平台暂未提供 acs_client_core 动态库（仅 Windows 构建）");
    return b;
#endif
}

} // namespace

static const char *emptyJson = "{}";

bool Acs::available()
{
    return bindings().ok;
}

QString Acs::lastError()
{
    return bindings().error;
}

QString Acs::init()
{
    auto &b = bindings();
    if (!b.ok) {
        return b.error;
    }
    const unsigned got = b.abiVersion();
    if (got != ABI_EXPECTED) {
        return QStringLiteral("DLL ABI 版本不匹配：宿主期望 %1，实际 %2（请同步升级客户端与 DLL）")
            .arg(ABI_EXPECTED)
            .arg(got);
    }
    return {};
}

QString Acs::version()
{
    auto &b = bindings();
    if (!b.ok || !b.version) {
        return {};
    }
    return QString::fromUtf8(b.version());
}

unsigned Acs::abiVersion()
{
    auto &b = bindings();
    return b.ok ? b.abiVersion() : 0u;
}

void Acs::freeString(char *p)
{
    auto &b = bindings();
    if (b.ok && b.freeString && p) {
        b.freeString(p);
    }
}

QJsonObject Acs::call(const QString &method, const QJsonObject &req, QString *error,
                      QString *errorCode)
{
    auto &b = bindings();
    if (error) {
        error->clear();
    }
    if (errorCode) {
        errorCode->clear();
    }
    if (!b.ok || !b.call) {
        if (error) {
            *error = b.error.isEmpty() ? QStringLiteral("acs_client_core 未就绪") : b.error;
        }
        return {};
    }
    const QByteArray m = method.toUtf8();
    const QByteArray r = req.isEmpty() ? QByteArray(emptyJson)
                                       : QJsonDocument(req).toJson(QJsonDocument::Compact);
    char *out = b.call(m.constData(), r.constData());
    if (!out) {
        if (error) {
            *error = QStringLiteral("后端返回空指针（内部错误）");
        }
        return {};
    }
    const QByteArray bytes(out);
    freeString(out);

    QJsonParseError perr{};
    const QJsonDocument doc = QJsonDocument::fromJson(bytes, &perr);
    if (perr.error != QJsonParseError::NoError || !doc.isObject()) {
        if (error) {
            *error = QStringLiteral("后端响应不是合法 JSON：%1").arg(perr.errorString());
        }
        return {};
    }
    const QJsonObject obj = doc.object();
    if (obj.value(QStringLiteral("ok")).toBool()) {
        return obj.value(QStringLiteral("data")).toObject();
    }
    if (error) {
        *error = obj.value(QStringLiteral("error")).toString();
    }
    if (errorCode) {
        *errorCode = obj.value(QStringLiteral("code")).toString();
    }
    return {};
}

QString Acs::errorText(const QString &error, const QString &code)
{
    if (code.isEmpty()) {
        return error;
    }
    return QStringLiteral("[%1] %2").arg(code, error);
}
