# 💰 A€ — Alpha Coin 中心化数字货币结算系统

> **ACSystem**：为 Minecraft 服务器组织 **AEU（Alpha Economy Union）** 提供可审计、可签名的 A€ 结算基础设施。
> Rust workspace（核心库 / 中心服务器 / 桌面钱包 · Tauri 2）。当前版本 **v3.0.0**。

---

## ✨ 特性总览

- 🏦 **中心化结算**：发行权收归理事会，中心密钥由理事长口令 AES-GCM 加密保管。
- 🔗 **防双花**：SQLite（WAL）+ `BEGIN IMMEDIATE`，每账户哈希链（`last_tx_hash`）环环相扣。
- ✍️ **防假币**：每笔交易发送方 **ed25519 签名** + 中心签名；接收方确认 / 拒收后入账。
- ⚡ **交易即发**：客户端验口令并签名后直接提交中心，无二次确认；Rejected / Error 不计余额。
- 🖥️ **桌面钱包（Tauri 2）**：银行风格多级菜单（交易 / 账单 / 个人 / 设置）+ 多套主题与明暗观感。
- 🔑 **密钥体系**：GnuPG（ed25519）签发身份；私钥经口令加密后才上链 / 存中心，口令不落盘、不传明文。
- 🔁 **自动同步**：登录后立即同步，之后每 3 分钟一次，并可手动「刷新」。
- 📊 **账单视图**：流水图 + 月度 · 总账（分层折叠），展示收 / 支 / 净额。
- 🗂️ **系统账本**：系统账户不登录客户端，改由后台 `/sys` 进入，服务端持密钥构建并签名。
- 🛡️ **安全加固**：请求体 4MB、超时 30s、隐藏 Server 头、安全响应头、审计留痕、无硬编码密钥。

---

## 🏗️ 架构与端口

```
桌面 / 钱包客户端 (Tauri 2) ──https──► 穿透服务商边缘 :443 (AutoTLS 终止)
                                             │ 内网穿透隧道（frp 等，自行部署）
                                             ▼
                                     acs-server 公开 API  :9600 (0.0.0.0)
                                       /api/client/*   /api/sync   /api/legal/*
                                       /api/client/update/*   /api/status
                                             ▲
                                     (ed25519 签名校验 · 同步免 apikey)
管理员 (浏览器) ────────────────────────►  acs-server 管理后台 :9680 (0.0.0.0 · 已开放公网)
                                             /login /root /finance /sys + /api/admin/*
                                             （含系统账本 /api/admin/sys/*）
```

| 服务 | 端口 | 绑定 | 暴露内容 |
|---|---|---|---|
| **公开 API** | **9600** | `0.0.0.0` | 仅 client：`/api/client/*`、`/api/sync`、`/api/legal/{doc}`、`/api/client/update/*`、`/api/status`（同步免 apikey，无网页、无管理） |
| **后台管理** | **9680** | `0.0.0.0`（**已开放公网**） | 网页后台 + 管理 API `/api/admin/*`、`/api/accounts`、`/api/stats`、`/api/audit`、`/api/members`、系统账本 `/sys` + `/api/admin/sys/*` |

> ⚠️ **9680 管理后台现已开放公网**：请务必使用强口令、限制来源 IP，并关注审计日志（详见「🔐 安全模型」）。

---

## 🧩 Workspace 模块

| Crate | 角色 | 说明 |
|---|---|---|
| **acs-core** | 核心库 | 模型 / SQLite / 账户 / 交易 / GnuPG / 配置 / 错误（`rlib` + `cdylib`） |
| **acs-server** | 中心服务器 | axum 0.8 双端口：公开 API + 网页管理后台 |
| **acs-client** | 桌面钱包 | Tauri 2 应用 + CLI 子命令；数据于 `~/.alpha_dir/acs-client` |

> **信任模型**：中心 > 本地。中心权威结算，客户端直接从中心同步（无镜像中间层）。
> **私钥安全**：私钥由钱包口令加密（校验 `$salt$sha256`）后才上链 / 存中心，口令不落盘、不传明文，解密导入在本地完成。

---

## 🚀 快速开始

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
#   [acs-server] 后台管理: http://0.0.0.0:9680
```

首次启动会：迁移旧库 → 按密码策略种子管理员 / 系统账户（见下）。

> **密码策略**
> - **无 `~/.alpha_dir/acs-server/.env`**：创建默认 `admin`（root 角色），随机密码输出到 `~/.alpha_dir/acs-server/SYSTEM_LOGIN_PASSWORDS.txt`；**不创建系统账户**。
> - **有 `~/.alpha_dir/acs-server/.env`**：自动**禁用默认 admin**；管理员按 `ACS_ADMIN_ACCOUNTS`（`uid:role:密码`）、系统账户按 `ACS_SYSTEM_ACCOUNTS`（`uid:密码`）创建；密码**只存哈希**、不输出明文。格式参考仓库根 `.env.example`。

### 3. 客户端（Alpha Wallet · 桌面版）

```powershell
# 启动桌面钱包（Tauri 2，无需浏览器）
acs-client
# 首次：① 配置中心服务器（留空默认 https://acsystem.maxshin.top）→ ② 登录 / 注册
```

**多级菜单**：交易（转账 / 待收箱）、账单（流水图 / 月度 · 总账）、个人（账户信息 / 余额 / 退出登录 / 注销账户）、设置（界面个性化 / 中心地址 / 使用教程 / 检查更新 / 关于软件）。登录后自动同步（每 3 分钟一次）。

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

### 5. 数据修复

Rejected / Error / 异常金额等历史脏数据，由技术侧离线处理（读取数据库 → 清理异常交易 → 触发余额重算），无需在服务器安装 sqlite3 / python。

### 6. 运行测试

```powershell
cargo test -p acs-core
```

---

## 🌐 公网部署（内网穿透）

> 本项目**不依赖 nginx 反向代理**。对外访问通过内网穿透（如 frp）暴露公网，
> HTTPS 证书由穿透服务商提供的 **AutoTLS** 自动签发。
> **frp 的具体部署方式（frps 服务端 / frpc 客户端 / Token 认证 / 域名解析）请读者自行研究**，
> 本仓库只给出与 ACS 相关的接入要点。

### 1. 穿透策略

| 要点 | 做法 |
|---|---|
| **公开 API 9600** | 穿透 9600（ed25519 签名校验、同步免 apikey，无网页、无管理） |
| **管理后台 9680** | 现已开放公网；建议强口令 + 来源限制 + 审计（也可设 `ACS_ADMIN_BIND=127.0.0.1` 改回仅本机） |
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

## 🔐 安全模型

- **管理端暴露面**：9680 已开放公网，属高风险面；务必强口令、限制来源 IP、定期查审计日志。
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
| `ACS_ADMIN_BIND` | `127.0.0.1` | 后台管理监听地址（开放公网需设为 `0.0.0.0`） |

> 客户端数据目录固定为 `~/.alpha_dir/acs-client`（数据库 / gpg / 运行日志）。
> 管理员 / 系统账户的初始密码通过 `~/.alpha_dir/acs-server/.env` 定义（见「快速开始」密码策略）；登录后请立即修改。

---

## ❓ 常见问题

| 现象 | 处理 |
|---|---|
| 同步失败/连不上中心 | 先本地 `http://127.0.0.1:9600` 验证服务；再查 frp 进程/Token/域名解析 |
| 管理后台无法从公网访问 | 检查 `ACS_ADMIN_BIND=0.0.0.0`、防火墙与穿透是否放行 9680 |
| 登录后账本一直“尚未刷新” | 可能中心不可达或未登录；点顶栏「刷新」，并查日志定位 |
| 装完没弹 Gpg4win 向导 / 提示缺 gpg | 已装则跳过；未装则从 `{app}\tools\gpg4win-5.1.0.exe` 手动运行安装（勾选 GnuPG 核心 + Kleopatra） |
| 注册 Country/Company 提示"未认定" | 需根管理员/金融部先在后台「成员认定」添加该国家/企业 |
| 注销账户后无法登录 | 正常：已注销账户状态为 `Deleted`，中心保留账本供审计，不可再登录 |

---

## 📁 目录结构

```
ACSystem/
├── Cargo.toml              # workspace（acs-core / acs-server / acs-client）
├── acs-core/               # 核心库（rlib + cdylib）：模型/SQLite/账户/交易/GPG/协议/日志
├── acs-server/             # 中心服务器（axum 双端口 + 网页管理后台 + 系统账本 /sys）
├── acs-client/             # Tauri 2 桌面钱包（多级菜单 + 主题）/ CLI；前端资源内嵌于 exe
├── deploy/
│   └── certs/generate.ps1  # 本地证书生成（TCP 透传端到端加密时用）
├── packaging/              # 安装包脚本（.iss + Gpg4win 安装器；本地保留，不入库）
└── .gitignore              # 敏感文件一律不提交
```

---

## 📄 License

见仓库根目录 `LICENSE`。
