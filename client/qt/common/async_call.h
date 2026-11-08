// 异步 ABI 调用：把阻塞的 Acs::call（网络/磁盘）放到后台线程执行，
// 期间在所属窗口显示加载遮罩（含不确定进度条动画），完成后回 UI 线程回调。
#pragma once

#include <QJsonObject>
#include <QString>

#include <functional>

class QWidget;

namespace acs {

using AsyncCallback =
    std::function<void(const QJsonObject &data, const QString &error, const QString &code)>;

/// `context` 用于定位加载遮罩的父窗口与 UI 线程回调（窗口关闭后自动丢弃回调）。
void runAsync(const QString &method,
              const QJsonObject &req,
              QWidget *context,
              const QString &busyText,
              AsyncCallback cb);

} // namespace acs
