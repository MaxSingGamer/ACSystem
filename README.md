# 💰 A€ — Alpha Coin 中心化数字货币结算系统

> **ACSystem**：为 Minecraft 服务器组织 **AEU（Alpha Economy Union）** 提供可审计、可签名的 A€ 结算基础设施。

**当前版本：v4.0.0**

---

## 架构总览

```
                ┌───────────────────────────────┐
   QT 客户端  →  │  AlphaCoinServer（中心服务）   │  ← 服务中心面板 AlphaCoinServerPanel
  AlphaWallet   │  · 公开 API（ed25519 签名）    │     · 面板登录 / TOTP / Passkey
  AlphaCompany  │  · 网页管理后台（Root/Admin）  │     · 多服务启停 / 日志 / AlphaTar 包
  Wallet        │  · 系统账本 /sys（主密钥解锁）  │     · 双域名（接口 / 后台）管理
                └───────────────────────────────┘
```

- **信任模型**：中心权威结算；客户端从中心同步（无中间镜像层）
- **私钥**：钱包口令加密后本地保存，口令不落盘、不上传；签名在本地完成
- **服务端主密钥**：无任何 key 文件；每个 Root 一份「口令包裹的拷贝」存库，Root 登录时内存解锁（未解锁相关接口返回 409）
- **面板安全（v4.0.0）**：单一后台账户（默认 `root` / `ChangeMe123!`，硬编码，**不写入 .env**），首次登录强制同时改用户名与密码；账户安全操作需再次验证密码；可选 TOTP（最多 3 个，开启后密码登录强制 2FA）与 Passkey 一键登录

---

## 仓库结构

```
ACSystem/
├── core/                 # acs-core：核心库（rlib + cdylib）
│                         #   模型 / SQLite / 账户 / 交易 / GnuPG / 协议 / 日志 / 口令哈希
├── server/               # AlphaCoinServer.exe：中心服务（axum 双端口 + 网页后台）
├── panel/                # AlphaCoinServerPanel.exe：服务中心面板（独立 Web 面板）
├── client/
│   ├── core/             # acs-client-core → acs_client_core.dll（QT 经 C ABI 调用）
│   ├── qt/               # AlphaWallet / AlphaCompanyWallet（QT 6 客户端，Citi 风主题）
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
| `target/release/AlphaCoinClientCli.exe` | CLI（仅本地构建，**不随 Release 发布**） |
| `dist/AlphaWallet_v4.0.0_windows-x64.exe` | 个人客户端安装包（AlphaWallet，含 Qt 运行库） |
| `dist/AlphaCompanyWallet_v4.0.0_windows-x64.exe` | 企业客户端安装包（AlphaCompanyWallet，含 Qt 运行库） |
| `dist/AlphaCoinServer_v4.0.0_windows-x64.exe` | 服务端安装包（含面板） |

> **升级安装（v4.0.0 #12）**：三个安装包在开始安装前会自动检测并**静默卸载旧版本**
> （Inno 卸载器 `/VERYSILENT`，按 AppId + 显示名双重识别），再照常安装。
> 卸载只删除程序文件；`~/.alphacoinserver`、`~/.alphawallet`、`~/.alphacompanywallet`
> 下的数据（数据库 / 密钥 / 登录历史 / 日志）**完整保留**。
>
> **注意**：v4.0.0 客户端与服务端需**同时升级**（同步接口有协议变更，旧版本互不兼容）。

---

## 端口与配置

| 端 | 默认 | 说明 |
|---|---|---|
| 中心服务 · 服务接口（API）端口 | **手动配置**（无默认） | 客户端访问；对外只需暴露此端口 |
| 中心服务 · 后台管理端口 | **手动配置**（无默认） | 网页后台 + 管理 API |
| 服务中心面板 | `127.0.0.1:9600` | 安全默认只本机（D40）；须登录 |

一个中心服务有**两个域名**（v4.0.0 Panel #3）：

| 域名 | 用途 |
|---|---|
| **服务接口域名（api_domain）** | 唯一识别符；同时决定服务文件夹名；客户端 API 入口 |
| **后台管理域名（admin_domain）** | 网页入口，仅用于把访问者重定向到后台登录页 |

访问「服务接口域名」根路径会 302 到 `https://<后台管理域名>/login`；
若文件夹名与接口域名不一致（导入旧服务包时可能出现），面板会**要求选择**以哪一个为准。

配置：`~/.alphacoinserver/<服务接口域名>/.env`（由面板生成，含勿手改提示）；面板配置在
`~/.alphacoinserver/.env`（`ACS_PANEL_*`），面板账户在 `~/.alphacoinserver/panel-account.json`。
**客户端不需要 .env** —— 品牌/更新源/最短口令长度等由服务端 `GET /api/client/config` 白名单下发并本地缓存（离线可用）。

---

## 安全与部署（v4.0.0）

- **HTTPS 与外网暴露**：本版本不内建 TLS 终止，公网部署请在前面使用反向代理或内网穿透工具（如 AutoTLS）提供 HTTPS；后台/面板域名走同一入口，**不要直接暴露裸 HTTP 端口**。
- **登录与账户**：后台/面板登录带失败锁定与限流；面板可选 TOTP（最多 3 个）与 Passkey；Passkey 需要 HTTPS 域名，裸 IP 无法注册。
- **接口**：公开 API 使用账户签名同步、仅返回该账户相关数据；管理 API 需要 Bearer token；统一错误文案，不返回框架细节。
- **抗压**：按来源 IP 限流、在途请求上限与请求日志写入保护。可调参数：`ACS_RATE_LIMIT_RPS` / `ACS_RATE_LIMIT_BURST` / `ACS_ADMIN_RATE_LIMIT_RPS` / `ACS_ADMIN_RATE_LIMIT_BURST` / `ACS_MAX_INFLIGHT` / `ACS_LOG_MAX_LINES_PER_SEC`；反向代理后需正确传递 `X-Forwarded-For`（且只信任可信代理传入的值）。
- **兼容性**：v4.0.0 的同步接口为签名调用，**客户端与服务端需同时升级**；CLI 的 `sync` 需显式传 `--pass`。

---

## 客户端

| 程序 | 账户类型 | 说明 |
|---|---|---|
| **AlphaWallet** | 个人 / 国家 | 多中心选择（按服务名称区分）、登录/注册、转账、待收箱、流水 |
| **AlphaCompanyWallet** | 企业 / 银行 | 同上，类型互斥（D30） |

- 启动流程：选择中心服务 → 登录/注册 → 主界面（顶部品牌条 + 左侧导航 + 卡片内容）
- 界面：白卡片 + 海军蓝 `#003B70` + 品牌蓝 `#056DAE` 的金融风主题；页面切换淡入/滑入、余额滚动动效
- 数据目录：`~/.alphawallet\` / `~/.alphacompanywallet\`（每个中心一个子目录），中心历史 `central_server_history.db`
- CLI：`AlphaCoinClientCli.exe`（`login` / `sync --pass` / `transfer` / `config` 等子命令）

---

## 管理后台（网页）

| 页面 | 说明 |
|---|---|
| `/login` | 管理员登录（Root/Admin） |
| `/root` | 根管理员：状态 / **账户**（账本账户 + 管理员两栏）/ **我的**（自身信息 + 改自己密码）/ 安全（铸造/根密钥）/ 审计 |
| `/admin` | 普通管理员：状态 / 账本账户 / **我的** / 审计 |
| `/sys` | 系统账本账户登录（顶栏「🏦 系统账本账户登录」进入） |

- 根管理员在「账户 → 管理员」栏对每个管理员账户执行**重置密码**（需二次键入新密码）
- **任何场景改密码都要二次键入新密码**（管理员自身 / Root 重置 Admin / 系统账本口令，服务端强校验）
- 顶栏功能按钮（系统账本账户登录 / 我的 / 退出）统一字体与排版，退出带 🚪 符号

---

## 数据目录

```
~/.alphacoinserver/                  # 服务端（目录名固定不可改，D33）
├── .env                             # 面板自身配置（域名记录 + 端口）
├── panel-account.json               # 面板登录账户 + TOTP/Passkey（无明文口令）
├── servers.json                     # 已注册服务清单（api_domain / admin_domain / 端口 / 目录）
├── logs/YYMMDD-HH_mm_SS.alphalog     # 面板自身日志
└── <服务接口域名>/                    # 每个服务一套（由面板维护）
    ├── .env   alpha_center.db   logs/YYMMDD-HH_mm_SS.alphalog   gnupg/

~/.alphawallet/<中心>/wallet.db              # 个人版客户端
~/.alphacompanywallet/<中心>/wallet.db       # 企业版客户端
```

日志（v4.0.0 #7）：文件名统一 `YYMMDD-HH_mm_SS.alphalog`（本地时间），保留最近 30 份
（`ACS_LOG_KEEP_FILES`），口令与用户名自动打码。面板启动服务时通过 `-logfile` 指定确切日志文件，
**不再产生 `panel-spawn.log`** —— 日志只由 Server 自己写。

---

## 文档

协议与条例文档（docx/pdf/md）存放于仓库上一级的 `Documents/` 目录，不在本仓库内。

## 许可

MIT
