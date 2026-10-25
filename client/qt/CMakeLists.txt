# A€ QT 客户端（D6 / D12 / D42 / D43）
#
# 目标：
#   acs_abi        公共静态库（C++ 侧 C ABI 封装，UTF-8/JSON）
#   abi_smoke      控制台冒烟测试：验证 DLL 加载 + 4 个导出符号 + 基本方法
#   AlphaWallet    个人 + 国家客户端（QT）
#   AlphaBankmanage 企业（银行）客户端（QT）
#
# 构建（Windows / MSVC）：
#   cmake -S client/qt -B build/qt -G Ninja ^
#         -DCMAKE_PREFIX_PATH="<Qt>/6.x/msvc2022_64" ^
#         -DACS_CLIENT_CORE_DLL="<repo>/target/debug/acs_client_core.dll"
#   cmake --build build/qt
#
# 说明：AlphaWallet / AlphaBankmanage 的界面源文件在后续步骤加入；本文件先保证
# abi_smoke 可构建，作为 QT 环境与 DLL 集成的第一道验证。
cmake_minimum_required(VERSION 3.21)
project(AlphaCoinClients LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_AUTOMOC ON)
set(CMAKE_AUTORCC ON)
# QT 客户端窗口/任务栏图标（qrc 内嵌，见 win/icon.qrc）
# 不用 .ui 文件（全部手写布局）；关闭 AUTOUIC 避免误扫头文件报错
set(CMAKE_AUTOUIC OFF)

# 统一输出目录：exe 与 DLL 放一起，Windows 可直接按同目录找到依赖
set(ACS_QT_OUTPUT_DIR "${CMAKE_BINARY_DIR}/bin")
set(CMAKE_RUNTIME_OUTPUT_DIRECTORY "${ACS_QT_OUTPUT_DIR}")
set(CMAKE_ARCHIVE_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/lib")
set(CMAKE_LIBRARY_OUTPUT_DIRECTORY "${ACS_QT_OUTPUT_DIR}")

# Rust 后端 DLL：开发期显式传入；未传则尝试仓库默认的 target/debug
if(NOT DEFINED ACS_CLIENT_CORE_DLL)
  get_filename_component(ACS_REPO_ROOT "${CMAKE_CURRENT_SOURCE_DIR}/.." ABSOLUTE)
  set(ACS_CLIENT_CORE_DLL "${ACS_REPO_ROOT}/target/debug/acs_client_core.dll")
endif()
if(NOT EXISTS "${ACS_CLIENT_CORE_DLL}")
  message(WARNING "未找到 acs_client_core.dll：${ACS_CLIENT_CORE_DLL}\n"
                  "请先 `cargo build -p acs-client-core`，或用 -DACS_CLIENT_CORE_DLL=<path> 指定。")
endif()

find_package(Qt6 6.5 REQUIRED COMPONENTS Core Widgets Test)
qt_standard_project_setup()

# 资源编译器（rc.exe）需要能找到同目录的 app.manifest
set(CMAKE_RC_FLAGS "${CMAKE_RC_FLAGS} -I${CMAKE_CURRENT_SOURCE_DIR}/win")

# ---------------- 公共 ABI 封装 ----------------
add_library(acs_abi STATIC common/acs_abi.cpp common/acs_abi.h)
target_include_directories(acs_abi PUBLIC ${CMAKE_CURRENT_SOURCE_DIR}/common)
target_link_libraries(acs_abi PUBLIC Qt6::Core)
target_compile_definitions(acs_abi PRIVATE
  ACS_CLIENT_CORE_DLL_PATH="${ACS_CLIENT_CORE_DLL}")

# ---------------- ABI 冒烟测试（控制台） ----------------
add_executable(abi_smoke abi_smoke/main.cpp)
target_link_libraries(abi_smoke PRIVATE acs_abi Qt6::Core)

# 把 Rust DLL 复制到输出目录（exe 同目录可加载；注意必须放在 target 创建之后）
if(EXISTS "${ACS_CLIENT_CORE_DLL}")
  add_custom_command(TARGET abi_smoke POST_BUILD
    COMMAND ${CMAKE_COMMAND} -E copy_if_different "${ACS_CLIENT_CORE_DLL}" "${ACS_QT_OUTPUT_DIR}")
endif()

# ---------------- 共用 UI（登录/启动/主窗口） ----------------
add_library(acs_ui STATIC
  common/ui_common.cpp
  common/ui_common.h
  common/center_picker.cpp
  common/center_picker.h
  common/auth_widget.cpp
  common/auth_widget.h
  common/main_window.cpp
  common/main_window.h
)
target_link_libraries(acs_ui PUBLIC acs_abi Qt6::Widgets)

# ---------------- 两个 QT 客户端（D30：账户类型互相排斥） ----------------
add_executable(AlphaWallet WIN32 wallet/main.cpp win/app.rc win/icon.qrc)
target_link_libraries(AlphaWallet PRIVATE acs_ui Qt6::Widgets)

add_executable(AlphaBankmanage WIN32 bankmanage/main.cpp win/app.rc win/icon.qrc)
target_link_libraries(AlphaBankmanage PRIVATE acs_ui Qt6::Widgets)

# 把 Rust DLL 复制到输出目录（GUI 同样需要同目录可加载）
if(EXISTS "${ACS_CLIENT_CORE_DLL}")
  add_custom_command(TARGET AlphaWallet POST_BUILD
    COMMAND ${CMAKE_COMMAND} -E copy_if_different "${ACS_CLIENT_CORE_DLL}" "${ACS_QT_OUTPUT_DIR}")
  add_custom_command(TARGET AlphaBankmanage POST_BUILD
    COMMAND ${CMAKE_COMMAND} -E copy_if_different "${ACS_CLIENT_CORE_DLL}" "${ACS_QT_OUTPUT_DIR}")
endif()

# ---------------- Qt Test 自动化回归（D47） ----------------
add_executable(qt_test tests/test_abi.cpp)
target_link_libraries(qt_test PRIVATE acs_abi Qt6::Test Qt6::Core)
if(EXISTS "${ACS_CLIENT_CORE_DLL}")
  add_custom_command(TARGET qt_test POST_BUILD
    COMMAND ${CMAKE_COMMAND} -E copy_if_different "${ACS_CLIENT_CORE_DLL}" "${ACS_QT_OUTPUT_DIR}")
endif()