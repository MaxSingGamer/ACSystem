# A€ — Alpha Coin 中心化数字货币结算系统

> **ACSystem**：为 Minecraft 服务器组织 **AEU（Alpha Economy Union）** 提供可审计、可签名的 A€ 结算基础设施。
> Rust workspace，三个 crate：核心库 / 中心服务器 / 桌面钱包客户端（acs-mirror 只读镜像已于 v3.0.0 取消）。
> 当前版本 **v3.0.0**。

---

## 一、特性总览

- **中心化结算**：发行权收归理事会，中心密钥由理事长口令 AES-GCM 加密保管。
- **防双花**：SQLite（WAL）+ `BEGIN IMMEDIATE` 事务，每账户哈希链（`last_tx_hash`）环环相扣。
- **防假币**：每笔交易由发送方 **ed25519 签名** + 中心签名；接收方在「待收箱」确认或拒收后入账。
- **交易即发**：客户端验证口令并签名后**直接提交中心**，无二次确认流程；拒收（Rejected）/ 错误（Error）交易不计入余额。
- **桌面客户端（Tauri 2）**：现代银行风格桌面应用，深色侧边栏多级菜单（交易 / 账单 / 个人 / 设置）与功能页隔离；多套预设主题（经典蓝 / 翡翠绿 / 酒红 / 曜黑 / 深蓝），支持明暗观感。
- **协议与隐私同意**：登录 / 注册须先勾选同意《使用协议》与《隐私政策》（个人与企业使用协议不同）；未勾选前端直接拦截、不发请求，服务端亦校验并记录同意标识与上次登录时间。
- **多账户登录**：登录界面不展示本地历史记录，直接输入 UID+密码即可；本地有加密私钥缓存则直接解锁，否则自动向中心取回（跨设备恢复）。
- **密钥体系**：GnuPG（`gpg.exe`，ed25519）签发身份；账户公钥上链，私钥始终由你的口令加密——加密副本存中心可跨设备恢复，口令不落盘、不传明文。
- **自动更新**：客户端启动自动检查更新；**以中心为版本权威**——先向中心获取最新版本号，仅当 GitHub Release 恰为该最新版才走 GitHub，否则直连中心下载；服务端安装器内置同版本客户端安装包（`client-package/`），开箱即可下发；服务器以清单白名单 + sha256 + IP 限速防恶意下载。
- **余额自动重算**：每次读取 / 同步余额前，中心按已确认交易重算各账户余额（Mint/Issue 增发、Redeem 回收、Transfer 双向；Rejected / Error 不计），自动纠正历史结算差异。
- **自动同步 + 手动刷新**：登录后立即自动同步账本，之后每 3 分钟一次；顶栏与各页提供手动「刷新」按钮。
- **账单视图**：流水图（累计余额走势）与月度 · 总账（按月份折叠，无交易月份不显示，月内按日期再折叠），展示收 / 支 / 净额。
- **详细运行日志**：每次启动在数据目录 `logs/` 下新建 `{启动时间}.alphalog`，统一「时间 - [类型] 内容」，记录全操作 / 调用 / 通讯 / 输出 / 错误 / 输入；口令、密钥、用户目录名自动打码。
- **成员国家/企业认定**：管理员在后台认定 AEU 成员，客户端注册 Country / Company 只能从已认定列表选择（服务端二次校验）。
- **注销账户（双重）**：中心将状态改为 `Deleted`（账户与账本只读保留供审计、不可再登录）+ 本地删除记录与密钥。
- **双端口隔离**：公开 API（client）与网页管理后台分开监听，后台默认仅本机可达。
- **系统账本账户**：客户端不再登录 System 账户（服务端拦截）；系统账户（AESystem / AlphaEU / PreIssuedAccount 等）改由后台「系统账本账户登录」进入 `/sys`——界面与功能同客户端（余额 / 转账 / 待收箱 / 流水），由服务端持系统密钥构建并签名。
- **一键安装**：安装包内嵌 `gpg4win-5.1.0.exe`，装完自动启动 GnuPG 安装向导（默认 Program Files\GnuPG）。
- **HTTPS 就绪**：经内网穿透（如 frp）暴露公网，由穿透服务商 AutoTLS 提供证书；客户端走系统信任链校验。
- **安全加固**：请求体限 4MB、超时 30s、隐藏 Server 头、安全响应头（CSP / X-Frame-Options / nosniff / no-store）、请求级日志、管理操作审计留痕、源码无硬编码密钥。

---

## 二、架构

```
桌面 / 钱包客户端 (Tauri 2) ──https──► 穿透服务商边缘 :443 (AutoTLS 终止)
                                             │ 内网穿透隧道（frp 等，自行部署）
                                             ▼
                                     acs-server 公开 API  :9600 (0.0.0.0)
                                       /api/client/*   /api/sync   /api/legal/*
                                       /api/client/update/*   /api/status
                                             ▲
                                     (ed25519 签名校验 · 同步免 apikey)
内网管理员 (浏览器) ──────────────────────►  acs-server 管理后台 :9680 (127.0.0.1)
                                             /login /root /finance /sys + /api/admin/*
                                             （含系统账本 /api/admin/sys/*）
```

| 服务 | 默认端口 | 绑定 | 暴露内容 |
|---|---|---|---|
| **公开 API** | **9600** | `0.0.0.0` | 仅 client：`/api/client/*`、`/api/sync`、`/api/legal/{doc}`、`/api/client/update/*`、`/api/status`（同步免 apikey，无网页、无管理） |
| **后台管理** | **9680** | `127.0.0.1`（仅本机） | 网页后台 + 管理 API `/api/admin/*`、`/api/accounts`、`/api/stats`、`/api/audit`、`/api/members`、系统账本 `/sys` + `/api/admin/sys/*` |

> 对外只暴露 **9600**（经内网穿透）；9680 管理端**不开放公网**，管理员在本机访问，或经 SSH/RDP 隧道访问。

---

## 三、Workspace 模块

| Crate | 角色 | 说明 |
|---|---|---|
| **acs-core** | 核心库 | 数据模型 / SQLite / 账户 / 交易 / GnuPG / 配置 / 错误；产出 `rlib` + `cdylib`(dll) |
| **acs-server** | 中心服务器 | axum 0.8，双端口：公开 API + 网页管理后台 |
| **acs-client** | 桌面钱包 | Tauri 2 桌面应用（现代银行风格多级菜单 + 主题）+ CLI 子命令；多账户、账本与日志于 `~/.alpha_dir/acs-client` |

### 信任模型

```
中心 > 本地
```
中心权威结算；客户端本地保存账户、账本与日志，直接从中心同步（无镜像中间层）。

> **私钥安全**：私钥由你的钱包口令加密（口令校验用 `$salt$sha256`）后才上链/存中心，中心与网络均只见密文；
> 登录取回时服务端只校验口令哈希，解密与导入全程在本地完成，口令不落盘、不传明文。

---

## 四、快速开始

### 0. 依赖

- Rust（2024 edition，workspace `resolver = "2"`）
- GnuPG（`gpg.exe` 在 PATH 或由 `acs-core` 自动探测/内嵌）
- Windows / Linux / macOS（SQLite bundled）

### 1. 构建（release，含 LTO + strip）

```powershell
cargo build --release
```

### 2. 启动中心服务器

```powershell
cargo run -p acs-server
# 日志：
#   [acs-server] 公开 API（client）: http://0.0.0.0:9600
#   [acs-server] 后台管理（仅内网）: http://127.0.0.1:9680
```

首次启动会：迁移旧库 → 按密码策略种子管理员 / 系统账户（见下）。

> **密码策略**
> - **无 `~/.alpha_dir/acs-server/.env`**：创建默认 `admin`（root 角色），随机密码输出到 `~/.alpha_dir/acs-server/SYSTEM_LOGIN_PASSWORDS.txt`；**不创建系统账户**。
> - **有 `~/.alpha_dir/acs-server/.env`**：自动**禁用默认 admin**；管理员按 `ACS_ADMIN_ACCOUNTS`（`uid:role:密码`）、系统账户按 `ACS_SYSTEM_ACCOUNTS`（`uid:密码`）创建；密码**只存哈希**、不输出明文。格式参考仓库根 `.env.example`。

### 3. 客户端（Alpha Wallet · 桌面版）

```powershell
# 启动桌面钱包（Tauri 2，无需浏览器）
acs-client
# 首次：① 配置中心服务器（留空默认 https://acsystem.maxshin.top）→ ② 登录/注册（须勾选同意《使用协议》与《隐私政策》）
```

**多级菜单**：交易（转账 / 待收箱）、账单（流水图 / 月度 · 总账）、个人（账户信息 / 余额 / 退出登录 / 注销账户）、设置（界面个性化 / 中心地址 / 使用教程 / 用户协议 / 隐私政策 / 开源协议 / 检查更新 / 关于软件）。登录后自动同步（每 3 分钟一次），启动时自动检查更新；个人与企业账户的《使用协议》不同，登录 / 注册前须阅读并勾选同意。

**CLI 子命令**（脚本/调试用）：
```powershell
acs-client new --uid Steve --email Steve@aeu.org --pass 'xxx' --server http://127.0.0.1:9600
acs-client status / sync / open / send / confirm / config
```

### 4. 管理后台（成员认定 / 审计 / 系统账本）

根管理员（9680）登录后（支持明/暗主题切换）：
- **系统账本**（顶栏入口 `/sys`）：选择系统账户进入账本 → 余额 / 转账 / 待收箱（确认·拒收）/ 流水 / 退出账本
- **账户** → 查询/冻结账户、管理后台管理员、**AEU 成员国家 / 企业认定**（双列面板，添加/撤销/删除）
- **安全** → 铸造（发行）、根密钥解锁/导出
- **审计** → 管理日志、交易总账单（密码解锁）

金融部（finance）登录后：状态 / 企业账户（银行） / **成员企业认定** / 审计 / 系统账本。

### 5. 更新清单与数据修复

**客户端更新清单**：
- 服务端安装器**内置同版本客户端安装包**（`{安装目录}\client-package\update.json` + `acs-client-{版本}-windows-x64-setup.exe`），版本互相对应；服务端更新源优先读取此随包目录，运维也可放 `~/.alpha_dir/acs-server/updates/` 覆盖（格式参考仓库根 `acs-server/updates.example.json`）。
- 客户端启动自动检查更新：以中心为版本权威获取最新版本号，仅当 GitHub Release 恰为该最新版才走 GitHub，否则直连中心下载。

**数据修复**：Rejected / Error / 异常金额等历史脏数据，由技术侧离线处理（读取数据库 → 清理异常交易 → 触发余额重算），无需在服务器安装 sqlite3 / python。

### 6. 运行测试

```powershell
cargo test -p acs-core
```

---

## 五、公网部署（内网穿透 / frp）

> 本项目**不依赖 nginx 反向代理**。对外访问通过内网穿透（如 frp）暴露公网，
> HTTPS 证书由穿透服务商提供的 **AutoTLS** 自动签发。
> **frp 的具体部署方式（frps 服务端 / frpc 客户端 / Token 认证 / 域名解析）请读者自行研究**，
> 本仓库只给出与 ACS 相关的接入要点。

### 1. 穿透策略

| 要点 | 做法 |
|---|---|
| **暴露哪个端口** | 只穿透 **9600**（公开 API：ed25519 签名校验、同步免 apikey，无网页、无管理） |
| **管理后台 9680** | **绝不穿透**，保持 `127.0.0.1` 仅本机；远程管理走 SSH/RDP 隧道 |
| **证书** | 用穿透服务商 AutoTLS（最简单）；或 TCP 透传 + 本地证书（`deploy/certs/generate.ps1`） |
| **客户端** | `--server https://<穿透域名>`，走系统信任链校验，不跳过 |

> 实况参考：本项目公网示例 `https://acsystem.maxshin.top`（LoliaFRP 隧道 + AutoTLS）绑定到本地 `127.0.0.1:9600`，
> 客户端 `config --server https://acsystem.maxshin.top` 即可公网同步。

### 2. 两种加密模式（选一）

| 模式 | TLS 终止位置 | 穿透服务商能否看明文 | 证书来源 |
|---|---|---|---|
| 服务商托管 HTTPS（AutoTLS） | 服务商边缘节点 | 能（信任边界） | 服务商自动签发 |
| TCP 透传 + 本地证书 | 你的 acs-server | 不能（端到端加密） | `deploy/certs/generate.ps1` 自签，或 acme.sh/certbot |

> 即使走服务商 AutoTLS，交易安全性仍由应用层 **ed25519 签名 + 哈希链** 兜底，不依赖传输保密；
> 同步接口免 apikey，无需额外凭据。

### 3. 客户端配置

```powershell
acs-client config --server https://acs.aeu.org
```

若走 **TCP 透传 + 本地自签证书**，需把证书导入系统根（客户端用 Windows schannel 校验链）：

```powershell
certutil -addstore -f Root deploy/certs/cert.pem   # 需管理员
```

正式证书则无需导入。内网直连仍可用 `--server http://<server-ip>:9600`。

---

## 六、安全模型

- **管理端隔离**：9680 默认绑定 `127.0.0.1`，公网不可达；对外只暴露 9600 的 client 端点。
- **穿透信任边界**：若用服务商托管 HTTPS（AutoTLS），穿透服务商处于 TLS 终止点、能看到交易明文；如需端到端保密，用 TCP 透传 + 本地证书（隧道只搬运加密字节）。同步接口免 apikey。
- **私钥托管**：中心只存口令加密的私钥副本与口令哈希（`$salt$sha256`）；口令不明文存储、不传输，登录取回仅返回密文私钥，解密导入在本地完成。
- **交易签名链**：发送方 ed25519 签名 → 中心验签并加签 → 接收方确认 → 写入双方哈希链。
- **发行权**：收归理事会；中心密钥由理事长口令 AES-GCM 加密保管，`gpg.exe`（ed25519）签发身份。
- **服务加固**：请求体 4MB、超时 30s、隐藏 Server 头、安全响应头（`Content-Security-Policy` / `X-Frame-Options: DENY` / `X-Content-Type-Options: nosniff` / `Referrer-Policy: no-referrer` / `Cache-Control: no-store`）、审计留痕。
- **仓库安全**：`.gitignore` 排除私钥（`*.key`/`*.asc`）、数据库、`alpha_dir/`、`target/`、`.env`；
  默认密码不硬编码（环境变量 / 随机生成）。

### 环境变量

| 变量 | 默认 | 说明 |
|---|---|---|
| `ACS_DATA_DIR` | `~/.alpha_dir/acs-server` | 服务器数据目录（数据库 / gpg / 系统账户密钥统一存放） |
| `ACS_PUBLIC_PORT` | `9600` | 公开 API 端口 |
| `ACS_PUBLIC_BIND` | `0.0.0.0` | 公开 API 监听地址 |
| `ACS_ADMIN_PORT` | `9680` | 后台管理端口 |
| `ACS_ADMIN_BIND` | `127.0.0.1` | 后台管理监听地址（保持本机即不开放公网） |

> 客户端数据目录固定为 `~/.alpha_dir/acs-client`（数据库 / gpg / 运行日志，每次启动在 `logs/` 下新建 `{启动时间}.alphalog`）。
> 管理员 / 系统账户的初始密码通过 `~/.alpha_dir/acs-server/.env` 定义（见「快速开始」密码策略）；登录后请立即修改。

---

## 七、常见问题

| 现象 | 处理 |
|---|---|
| 同步失败/连不上中心 | 先本地 `http://127.0.0.1:9600` 验证服务；再查 frp 进程/Token/域名解析 |
| 管理后台公网访问不到 | 正常：9680 仅本机；远程管理请用 SSH/RDP 隧道 |
| 浏览器提示"不安全" | 自签名证书未信任：`certutil -addstore -f Root cert.pem` 或换正式证书 |
| 登录/注册提示“请先同意协议” | 需在登录/注册页勾选同意《使用协议》（个人/企业版不同）与《隐私政策》后才可提交 |
| client 连不上穿透域名 | 先本地 `http://127.0.0.1:9600` 验证服务正常；再查 frp 进程/Token/域名解析 |
| 登录后账本一直“尚未刷新” | 可能中心不可达或未登录；点顶栏「刷新」，并查 `.alphalog` 日志定位 |
| 装完没弹 Gpg4win 向导 / 提示缺 gpg | 已装则跳过；未装则从 `{app}\tools\gpg4win-5.1.0.exe` 手动运行安装（勾选 GnuPG 核心 + Kleopatra） |
| 注册 Country/Company 提示"未认定" | 需根管理员/金融部先在后台「成员认定」添加该国家/企业 |
| 注销账户后无法登录 | 正常：已注销账户状态为 `Deleted`，中心保留账本供审计，不可再登录 |
| git commit 报 `gpg failed to sign the data` | 本机 gpg 不可用：`git -c commit.gpgsign=false commit ...` |

---

## 八、目录结构

```
ACSystem/
├── Cargo.toml              # workspace（acs-core / acs-server / acs-client）
├── acs-core/               # 核心库（rlib + cdylib）：模型/SQLite/账户/交易/GPG/协议/日志
├── acs-server/             # 中心服务器（axum 双端口 + 网页管理后台 + 系统账本 /sys + 更新清单）
├── acs-client/             # Tauri 2 桌面钱包（多级菜单 + 主题）/ CLI；前端资源内嵌于 exe
├── deploy/
│   └── certs/generate.ps1  # 本地证书生成（TCP 透传端到端加密时用）
├── packaging/              # 安装包脚本（.iss + Gpg4win 安装器；本地保留，不入库）
└── .gitignore              # 敏感文件一律不提交
```

---

## License

见仓库根目录 `LICENSE`。
