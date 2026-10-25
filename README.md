# 💰 A€ — Alpha Coin 中心化数字货币结算系统

> **ACSystem**：为 Minecraft 服务器组织 **AEU（Alpha Economy Union）** 提供可审计、可签名的 A€ 结算基础设施。

**当前版本：v3.3.0**

---

## 架构总览

```
                ┌───────────────────────────────┐
   QT 客户端  →  │  AlphaCoinServer（中心服务）   │  ← 服务中心面板 AlphaCoinServerPanel
  AlphaWallet   │  · 公开 API（ed25519 签名）    │     · 多服务启停/日志/AlphaTar 包
  AlphaBankmg.  │  · 网页管理后台（Root/Admin）  │     · 127.0.0.1 单端口（默认 9600）
                │  · 系统账本 /sys（主密钥解锁）  │
                └───────────────────────────────┘
```

- **信任模型**：中心权威结算；客户端从中心同步（无中间镜像层）
- **私钥**：钱包口令加密后本地保存，口令不落盘、不上传；签名在本地完成
- **服务端主密钥**：无任何 key 文件；每个 Root 一份「口令包裹的拷贝」存库，Root 登录时内存解锁（未解锁相关接口返回 409）

---

## 仓库结构

```
ACSystem/
├── core/                 # acs-core：核心库（rlib + cdylib）
│                         #   模型 / SQLite / 账户 / 交易 / GnuPG / 协议 / 日志
├── server/               # AlphaCoinServer.exe：中心服务（axum 双端口 + 网页后台）
├── panel/                # AlphaCoinServerPanel.exe：服务中心面板（独立 Web 面板）
├── client/
│   ├── core/             # acs-client-core → acs_client_core.dll（QT 经 C ABI 调用）
│   ├── qt/               # AlphaWallet / AlphaBankmanage（QT 6 客户端，Citi 风主题）
│   └── cli/              # AlphaCoinClientCli.exe：运维/自动化 CLI
├── scripts/              # e2e 回归脚本（本地，不入库）
├── packaging/            # 安装包脚本（本地，不入库）
└── Cargo.toml            # workspace：core / server / panel / client/core / client/cli
```

---

## 构建

```bash
# 全部 Rust 产物（服务端、面板、客户端 DLL、CLI）
cargo build --release --workspace

# QT 客户端 + 客户端安装包（需 Qt 6.12 MSVC 2022 + CMake + Inno Setup）
pwsh -File client/qt/build.ps1 -Release

# 服务端安装包（需已构建 release 产物）
& "C:\Program Files\Inno Setup 6\ISCC.exe" packaging/acs-server-setup.iss
```

产物：

| 产物 | 说明 |
|---|---|
| `target/release/AlphaCoinServer.exe` | 中心服务 |
| `target/release/AlphaCoinServerPanel.exe` | 服务中心面板 |
| `target/release/acs_client_core.dll` | 客户端业务 DLL（QT 通过 C ABI 调用） |
| `target/release/AlphaCoinClientCli.exe` | CLI |
| `dist/AlphaCoinClient_v3.3.0_windows-x64.exe` | 客户端安装包（含 Qt 运行库） |
| `dist/AlphaCoinServer_v3.3.0_windows-x64.exe` | 服务端安装包（含面板） |

---

## 端口与配置

| 端 | 默认 | 说明 |
|---|---|---|
| 中心服务 · 公开 API | **手动配置**（无默认） | 客户端访问；对外只需暴露此端口 |
| 中心服务 · 管理后台 | `127.0.0.1:9680` | 网页后台 + 管理 API |
| 服务中心面板 | `127.0.0.1:9600` | 安全默认只本机（D40） |

配置：`~/.alphacoinserver/<服务域名>/.env`（复制 `.env.example` 修改）；面板配置在 `~/.alphacoinserver/.env`（`ACS_PANEL_*`）。
**客户端不需要 .env** —— 品牌/更新源/最短口令长度等由服务端 `GET /api/client/config` 白名单下发并本地缓存（离线可用）。

---

## 客户端

| 程序 | 账户类型 | 说明 |
|---|---|---|
| **AlphaWallet** | 个人 / 国家 / 系统 | 多中心选择（按服务名称区分）、登录/注册、转账、待收箱、流水 |
| **AlphaBankmanage** | 企业 / 银行 | 同上，类型互斥（D30） |

- 启动流程：选择中心服务 → 登录/注册 → 主界面（顶部品牌条 + 左侧导航 + 卡片内容）
- 界面：白卡片 + 海军蓝 `#003B70` + 品牌蓝 `#056DAE` 的金融风主题；页面切换淡入/滑入、余额滚动动效
- 数据目录：`~/.alphawallet\` / `~/.alphabankmanage\`（每个中心一个子目录），中心历史 `central_server_history.db`
- CLI：`AlphaCoinClientCli.exe`（`login` / `sync` / `transfer` / `config` 等子命令）

---

## 数据目录

```
~/.alphacoinserver/                  # 服务端（三个目录名固定不可改，D33）
├── .env                             # 面板自身配置
├── servers.json                     # 已注册服务清单
├── central_server_history.db        # 根级中心历史
└── <服务域名>/                       # 每个服务一套
    ├── .env  central.db  logs/*.log  gnupg/

~/.alphawallet\<中心>/wallet.db      # 个人版客户端
~/.alphabankmanage\<中心>/wallet.db  # 企业版客户端
```

日志：`{启动时间戳ms}.log`，保留最近 30 份（`ACS_LOG_KEEP_FILES`），口令与用户名自动打码。

---

## 文档

协议与条例文档（docx/pdf/md）存放于仓库上一级的 `Documents/` 目录，不在本仓库内。

## 许可

MIT
