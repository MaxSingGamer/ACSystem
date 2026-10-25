// A€ 客户端 C ABI 封装（C++ / Qt 侧唯一入口）。
//
// 后端是 `acs_client_core.dll`（Rust，见 client-core/src/ffi.rs，D43）：
//   uint32_t acs_abi_version(void);
//   const char* acs_version(void);
//   char*    acs_call(const char* method, const char* req_json);
//   void     acs_string_free(char* p);
//
// 约定（务必遵守）：
// 1. 所有字符串都是 **UTF-8**（Qt 侧用 QString::fromUtf8 / toUtf8，禁止 toLocal8Bit）；
// 2. `acs_call` 的返回值由 Rust 分配，必须经 Acs::freeString 释放；
// 3. 返回永远是 JSON 信封：{ok:true,data:...} 或 {ok:false,code,error}；
// 4. 口令只在单次调用参数里出现，本封装不做任何缓存/落盘；
// 5. 启动时先校验 ABI 版本（Acs::abiVersion 与 Acs::ABI_EXPECTED 一致），不匹配直接报错。
#pragma once

#include <QJsonObject>
#include <QString>
#include <QStringList>

class Acs
{
public:
    /// 宿主期望的 ABI 版本（与 client-core/src/ffi.rs::ABI_VERSION 同步）。
    static constexpr unsigned ABI_EXPECTED = 1;

    /// DLL 是否已成功加载（构建期由 CMake 传入 DLL 路径；运行时同目录或 PATH 可找到依赖）。
    static bool available();

    /// 初始化：加载 DLL、校验 ABI 版本。返回空串=成功；否则为错误信息（含修复建议）。
    static QString init();

    /// 后端产品版本（acs_version）。
    static QString version();

    /// 后端 ABI 版本。
    static unsigned abiVersion();

    /// 错误信息（init 失败时用于界面提示）。
    static QString lastError();

    /// 调用一个方法。`req` 为 JSON 对象（可为空对象）。
    /// 成功返回 data 对象；失败时 error 非空（携带 code 与 error 文本）。
    static QJsonObject call(const QString &method, const QJsonObject &req = {},
                            QString *error = nullptr, QString *errorCode = nullptr);

    /// 便捷：把错误信封转成可展示的字符串。
    static QString errorText(const QString &error, const QString &code);

private:
    static void freeString(char *p);
};
