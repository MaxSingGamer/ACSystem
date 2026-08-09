# A€ — Alpha Coin 中心化数字货币结算系统

> **ACSystem**：为 Minecraft 服务器组织 **AEU（Alpha Economy Union）** 提供可审计、可签名的 A€ 结算基础设施。
> Rust workspace，四个 crate：核心库 / 中心服务器 / 钱包客户端 / 只读镜像。

---

## 一、特性总览

- **中心化结算**：发行权收归理事会，中心密钥由理事长口令 AES-GCM 加密保管。
- **防双花**：SQLite（WAL）+ `BEGIN IMMEDIATE` 事务，每账户哈希链（`last_tx_hash`）环环相扣。
- **防假币**：每笔交易由发送方 **ed25519 签名** + 中心签名，双方确认（`tx_confirmations`）后才入账。
- **密钥体系**：GnuPG（`gpg.exe`，ed25519）签发身份密钥；账户公钥上链，私钥只在客户端本地。
- **双端口隔离**：公开 API（client/mirror）与管理后台（网页 + 管理 API）分开监听，后台默认仅本机可达。
- **只读镜像**：`acs-mirror` 增量同步中心账本，提供只读 HTTP 查询，不承载任何写操作。
- **HTTPS 就绪**：nginx 反向代理 + AutoTLS 自签证书生成脚本，客户端使用系统信任链校验（不跳过）。
- **安全加固**：请求体限 4MB、超时 30s、隐藏 Server 头、管理操作审计留痕、源码无硬编码密钥。

---

## 二、架构

```
公网客户端 (TUI 钱包 / 镜像)          ──https──►  nginx :443 (TLS 终止)
                                                    │ 反代
                                                    ▼
                                            acs-server 公开 API  :9600 (0.0.0.0)
                                              /api/client/*   /api/mirror/pull
                                                    ▲
                                            (apikey 认证 + ed25519 签名校验)
内网管理员 (浏览器) ───────────────────────►  acs-server 管理后台 :9680 (127.0.0.1)
                                              /login /root /finance + /api/admin/*
```

| 服务 | 默认端口 | 绑定 | 暴露内容 |
|---|---|---|---|
| **公开 API** | **9600** | `0.0.0.0` | 仅 client / mirror：`/api/client/*`、`/api/mirror/pull`（apikey 认证，无网页、无管理） |
| **后台管理** | **9680** | `127.0.0.1`（仅本机） | 网页后台 + 管理 API `/api/admin/*`、`/api/accounts`、`/api/stats`、`/api/audit`、`/api/members`、`/api/admin/mirror-keys` |

> 对外（nginx）只代理 **9600**；9680 管理端**不开放公网**，管理员在本机访问，或经 SSH/RDP 隧道访问。

---

## 三、Workspace 模块

| Crate | 角色 | 说明 |
|---|---|---|
| **acs-core** | 核心库 | 数据模型 / SQLite / 账户 / 交易 / GnuPG / 配置 / 错误；产出 `rlib` + `cdylib`(dll) |
| **acs-server** | 中心服务器 | axum 0.8，双端口：公开 API + 网页管理后台 |
| **acs-client** | 钱包客户端 | ratatui TUI（OpenCode 风格）+ CLI 子命令；本地私钥于 `~/.alpha_dir` |
| **acs-mirror** | 只读镜像 | 增量同步 + 只读 HTTP 查询（默认 9090） |

### 信任模型

```
中心 > 本地 > 镜像
```
中心权威结算；客户端本地保存私钥与交易记录；镜像只读缓存，供只读查询。

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
（`PreIssuedAccount`/`AESystem`/`AlphaEU`，导出私钥到 `./alpha_dir`）。

> 管理后台默认密码从环境变量 `ACS_ADMIN_PASSWORD` 读取；**未设置则随机生成 16 位强密码**并打印到日志。

### 3. 客户端（Alpha Wallet）

```powershell
# 创建钱包（首次使用，交互或全参数）
acs-client new --uid Steve --email Steve@aeu.org --pass 'xxx' --server http://127.0.0.1:9600 --apikey mir-xxx

# 常用子命令
acs-client status                          # 钱包状态
acs-client sync                            # 从中心拉取一次
acs-client open                            # 在中心开立账户（上传公钥）
acs-client send <UID> <金额> --pass 'xxx'  # 本地签名一笔转账（写入 outbox）
acs-client submit                          # 提交 outbox 交易到中心
acs-client confirm --pass 'xxx'            # 确认/拒绝待确认交易（接收方）
acs-client config --server <url> --apikey <key>   # 运行期修改中心地址/apikey

# 不带子命令 → 进入 TUI（状态栏 / 导航 / 底部命令栏 / 常驻帮助）
acs-client
```

### 4. 镜像

```powershell
acs-mirror config --server http://127.0.0.1:9600 --apikey mir-xxx
acs-mirror sync                    # 拉取增量账本与账户快照
acs-mirror status                  # 同步状态
acs-mirror serve --port 9090       # 只读 HTTP 查询服务
```

> 镜像 apikey 在管理后台（9680）`/api/admin/mirror-keys` 生成。

### 5. 运行测试

```powershell
cargo test -p acs-core
```

---

## 五、HTTPS 部署（nginx + AutoTLS）

### 1. 生成自签证书（无需 OpenSSL，PowerShell 5.1+）

```powershell
cd deploy/certs
powershell -ExecutionPolicy Bypass -File generate.ps1 -DnsName acs.aeu.org -InstallTrust
```

生成 `deploy/certs/cert.pem` + `key.pem`。正式环境可用 acme.sh/certbot 覆盖这两个文件（或改 nginx 路径）。

### 2. 安装 nginx for Windows

1. 下载 <https://nginx.org/en/download.html>，解压到 `C:\nginx`。
2. 将 `deploy/nginx/nginx-acs.conf` 覆盖到 `C:\nginx\conf\nginx.conf`，修改 `server_name` 与证书路径。
3. 启动：

```powershell
C:\nginx\nginx.exe
C:\nginx\nginx.exe -s reload
```

> nginx 将 `location /` 反代到 `127.0.0.1:9600`；80 自动跳转 443；9680 管理端不经过 nginx。

### 3. 客户端走 HTTPS（不跳过校验）

```powershell
acs-client config --server https://acs.aeu.org --apikey mir-xxx
acs-mirror config --server https://acs.aeu.org --apikey mir-xxx
```

自签名证书需导入系统根（client/mirror 用 Windows schannel 校验链）：

```powershell
certutil -addstore -f Root deploy/certs/cert.pem   # 需管理员
```

正式证书则无需导入。内网直连可用 `--server http://<server-ip>:9600`。

---

## 六、安全模型

- **管理端隔离**：9680 默认绑定 `127.0.0.1`，公网不可达；对外只暴露 9600 的 client/mirror 端点。
- **交易签名链**：发送方 ed25519 签名 → 中心验签并加签 → 接收方确认 → 写入双方哈希链。
- **发行权**：收归理事会；中心密钥由理事长口令 AES-GCM 加密保管，`gpg.exe`（ed25519）签发身份。
- **服务加固**：请求体 4MB、超时 30s、隐藏 Server 头、审计留痕。
- **仓库安全**：`.gitignore` 排除私钥（`*.key`/`*.asc`）、数据库、`alpha_dir/`、`target/`、`.env`；
  默认密码不硬编码（环境变量 / 随机生成）。

### 环境变量

| 变量 | 默认 | 说明 |
|---|---|---|
| `ACS_DATA_DIR` | `%TEMP%\acs-server-data` | 服务器数据目录 |
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
| client/mirror 报 403 | apikey 无效：在管理后台（9680）`/api/admin/mirror-keys` 重新生成 |
| 管理后台公网访问不到 | 正常：9680 仅本机；远程管理请用 SSH/RDP 隧道 |
| 浏览器提示"不安全" | 自签名证书未信任：`certutil -addstore -f Root cert.pem` 或换正式证书 |
| nginx 报证书路径错误 | 检查 nginx.conf 的 `ssl_certificate*` 路径 |
| git commit 报 `gpg failed to sign the data` | 本机 gpg 不可用：`git -c commit.gpgsign=false commit ...` |

---

## 八、目录结构

```
ACSystem/
├── Cargo.toml              # workspace（acs-core/server/client/mirror）
├── acs-core/               # 核心库（rlib + cdylib）
├── acs-server/             # 中心服务器（双端口 axum）
├── acs-client/             # 钱包 TUI / CLI
├── acs-mirror/             # 只读镜像
├── deploy/
│   ├── certs/generate.ps1  # AutoTLS 证书生成（PowerShell 5.1+）
│   └── nginx/nginx-acs.conf# nginx 反代配置（80→443，代理 9600）
└── .gitignore              # 敏感文件一律不提交
```

---

## License

见仓库根目录 `LICENSE`。
