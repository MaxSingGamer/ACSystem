# A€ — Alpha Coin 中心化数字货币结算系统

> **ACSystem**：为 Minecraft 服务器组织 **AEU（Alpha Economy Union）** 提供可审计、可签名的 A€ 结算基础设施。
> Rust workspace，四个 crate：核心库 / 中心服务器 / 钱包客户端 / 只读镜像。
> 当前版本 **v2.0.0**。

---

## 一、特性总览

- **中心化结算**：发行权收归理事会，中心密钥由理事长口令 AES-GCM 加密保管。
- **防双花**：SQLite（WAL）+ `BEGIN IMMEDIATE` 事务，每账户哈希链（`last_tx_hash`）环环相扣。
- **防假币**：每笔交易由发送方 **ed25519 签名** + 中心签名，双方确认（`tx_confirmations`）后才入账。
- **多账户登录**：一个钱包可登录多个账户互不干扰；登录界面直接输入 UID+密码（本地记录不在界面展示），本地有加密私钥缓存直接解锁，否则自动向中心取回（跨设备恢复）。
- **密钥体系**：GnuPG（`gpg.exe`，ed25519）签发身份；账户公钥上链，私钥始终由你的口令加密——加密副本存中心可跨设备恢复，口令不落盘、不传明文。
- **Web GUI 客户端**：启动本地 Web 服务并自动打开浏览器，全中文鼠标操作（登录/注册/菜单/表单）；关闭网页自动退出进程。
- **弹窗式状态提醒**：同步/转账/提交等结果以居中弹窗展示，自动换行。
- **成员国家/银行认定**：管理员在后台认定 AEU 成员国家/银行，客户端注册 Country/Bank 账户只能从下拉列表选择已认定成员（服务端二次校验）。
- **注销账户（双重）**：中心将账户状态改为 `Deleted`（账户与账本只读保留供审计，不可再登录）+ 本地删除记录与密钥；危险操作有明确提示。
- **双端口隔离**：公开 API（client/mirror）与管理后台（网页 + 管理 API）分开监听，后台默认仅本机可达。
- **社区化同步**：client/mirror 免 apikey，自动测速选择最快镜像源。
- **一键安装**：client / server 安装包内嵌 `gpg4win-5.1.0.exe`，装完自动启动安装向导由用户手动安装（默认路径 Program Files\GnuPG），无需联网下载。
- **只读镜像**：`acs-mirror` 增量同步中心账本，提供只读 HTTP 查询，不承载任何写操作。
- **HTTPS 就绪**：经内网穿透（如 frp）暴露公网，由穿透服务商提供 AutoTLS 证书；也可 TCP 透传 + 本地证书实现端到端加密，客户端使用系统信任链校验（不跳过）。
- **安全加固**：请求体限 4MB、超时 30s、隐藏 Server 头、安全响应头（CSP/X-Frame-Options/nosniff/no-store）、管理操作审计留痕、源码无硬编码密钥。

---

## 二、架构

```
公网客户端 (Web 钱包 / 镜像) ──https──► 穿透服务商边缘 :443 (AutoTLS 终止)
                                              │ 内网穿透隧道（frp 等，自行部署）
                                              ▼
                                      acs-server 公开 API  :9600 (0.0.0.0)
                                        /api/client/*   /api/mirror/*   /api/status
                                              ▲
                                      (ed25519 签名校验 · 同步免 apikey)
内网管理员 (浏览器) ───────────────────────►  acs-server 管理后台 :9680 (127.0.0.1)
                                              /login /root /finance + /api/admin/*
```

| 服务 | 默认端口 | 绑定 | 暴露内容 |
|---|---|---|---|
| **公开 API** | **9600** | `0.0.0.0` | 仅 client / mirror：`/api/client/*`、`/api/mirror/*`、`/api/status`（同步免 apikey，无网页、无管理） |
| **后台管理** | **9680** | `127.0.0.1`（仅本机） | 网页后台 + 管理 API `/api/admin/*`、`/api/accounts`、`/api/stats`、`/api/audit`、`/api/members`、`/api/admin/mirror-keys` |

> 对外只暴露 **9600**（经内网穿透）；9680 管理端**不开放公网**，管理员在本机访问，或经 SSH/RDP 隧道访问。

---

## 三、Workspace 模块

| Crate | 角色 | 说明 |
|---|---|---|
| **acs-core** | 核心库 | 数据模型 / SQLite / 账户 / 交易 / GnuPG / 配置 / 错误；产出 `rlib` + `cdylib`(dll) |
| **acs-server** | 中心服务器 | axum 0.8，双端口：公开 API + 网页管理后台 |
| **acs-client** | 钱包客户端 | 本地 Web GUI（自动开浏览器 + 全中文鼠标操作）+ CLI 子命令；多账户与私钥于 `~/.alpha_dir/acs-client` |
| **acs-mirror** | 只读镜像 | 增量同步 + 只读 HTTP 查询（默认 9090） |

### 信任模型

```
中心 > 本地 > 镜像
```
中心权威结算；客户端本地保存多账户与交易记录；镜像只读缓存，供只读查询。

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
#   [acs-server] 公开 API（client/mirror）: http://0.0.0.0:9600
#   [acs-server] 后台管理（仅内网）: http://127.0.0.1:9680
```

首次启动会：迁移旧库 → 种子默认根管理员 → 种子系统账户
（`PreIssuedAccount`/`AESystem`/`AlphaEU`，私钥统一导出到 `~/.alpha_dir/acs-server`）。

> 管理后台默认密码从环境变量 `ACS_ADMIN_PASSWORD` 读取；**未设置则随机生成 16 位强密码**并打印到日志。

### 3. 客户端（Alpha Wallet · Web GUI）

```powershell
# 启动 Web 钱包：自动打开浏览器（关闭网页自动退出）
acs-client
# 首次打开：① 配置中心服务器（留空默认 https://acsystem.maxshin.top）→ ② 登录/注册
```

**首次使用**：网页内第一步配置中心服务器；第二步登录（输入 UID+密码）或注册新账户（个人直接填 UID；国家/银行从已认定下拉列表选择）。

**CLI 子命令**（脚本/调试用）：
```powershell
acs-client new --uid Steve --email Steve@aeu.org --pass 'xxx' --server http://127.0.0.1:9600
acs-client status / sync / open / send / submit / confirm
```

### 4. 管理后台（成员认定 / 审计）

根管理员（9680）登录后：
- **账户** → 查询/冻结账户、管理后台管理员、**AEU 成员国家 / 银行认定**（双列面板，添加/撤销/删除）
- **安全** → 铸造（发行）、根密钥解锁/导出
- **镜像** → 镜像源 apikey、社区镜像登记
- **审计** → 管理日志、交易总账单（密码解锁）

金融部（finance）登录后：状态 / 银行账户 / **成员企业认定** / 审计。

### 5. 镜像

```powershell
acs-mirror config --server http://127.0.0.1:9600
acs-mirror sync                    # 拉取增量账本与账户快照
acs-mirror status                  # 同步状态
acs-mirror serve --port 9090       # 只读 HTTP 查询服务
```

> 镜像源社区化：同步免 apikey，客户端启动时自动测速选择最快镜像源。

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
> 同步已社区化免 apikey，无需额外凭据。

### 3. 客户端配置

```powershell
acs-client config --server https://acs.aeu.org
acs-mirror config --server https://acs.aeu.org
```

若走 **TCP 透传 + 本地自签证书**，需把证书导入系统根（client/mirror 用 Windows schannel 校验链）：

```powershell
certutil -addstore -f Root deploy/certs/cert.pem   # 需管理员
```

正式证书则无需导入。内网直连仍可用 `--server http://<server-ip>:9600`。

---

## 六、安全模型

- **管理端隔离**：9680 默认绑定 `127.0.0.1`，公网不可达；对外只暴露 9600 的 client/mirror 端点。
- **穿透信任边界**：若用服务商托管 HTTPS（AutoTLS），穿透服务商处于 TLS 终止点、能看到交易明文；如需端到端保密，用 TCP 透传 + 本地证书（隧道只搬运加密字节）。同步接口已社区化免 apikey。
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
| `ACS_ADMIN_PASSWORD` | 随机生成 | 管理后台初始密码（登录后强制改密） |
| `ACS_ALPHA_DIR` | `~/.alpha_dir` | 客户端钱包目录 |
| `ACS_MIRROR_DIR` | `~/.alpha_mirror` | 镜像数据目录 |

---

## 七、常见问题

| 现象 | 处理 |
|---|---|
| 同步失败/连不上中心 | 先本地 `http://127.0.0.1:9600` 验证服务；再查 frp 进程/Token/域名解析 |
| 管理后台公网访问不到 | 正常：9680 仅本机；远程管理请用 SSH/RDP 隧道 |
| 浏览器提示"不安全" | 自签名证书未信任：`certutil -addstore -f Root cert.pem` 或换正式证书 |
| client/mirror 连不上穿透域名 | 先本地 `http://127.0.0.1:9600` 验证服务正常；再查 frp 进程/Token/域名解析 |
| 装完没弹 Gpg4win 向导 / 提示缺 gpg | 已装则跳过；未装则从 `{app}\tools\gpg4win-5.1.0.exe` 手动运行安装（勾选 GnuPG 核心 + Kleopatra） |
| 注册 Country/Bank 提示"未认定" | 需根管理员/金融部先在后台「成员认定」添加该国家/银行 |
| 注销账户后无法登录 | 正常：已注销账户状态为 `Deleted`，中心保留账本供审计，不可再登录 |
| git commit 报 `gpg failed to sign the data` | 本机 gpg 不可用：`git -c commit.gpgsign=false commit ...` |

---

## 八、目录结构

```
ACSystem/
├── Cargo.toml              # workspace（acs-core/server/client/mirror）
├── acs-core/               # 核心库（rlib + cdylib）
├── acs-server/             # 中心服务器（双端口 axum + 网页管理后台）
├── acs-client/             # Web GUI 钱包（内嵌网页 + 多账户登录）/ CLI
├── acs-mirror/             # 只读镜像
├── deploy/
│   ├── certs/generate.ps1  # 本地证书生成（TCP 透传端到端加密时用）
│   └── nginx/nginx-acs.conf# 可选：nginx 反代旧方案（非必须）
├── packaging/              # 安装包脚本（.iss + Gpg4win 安装器；本地保留，不入库）
└── .gitignore              # 敏感文件一律不提交
```

---

## License

见仓库根目录 `LICENSE`。
