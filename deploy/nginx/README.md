# A€ Alpha Coin System — 部署教程（双端口 + HTTPS 加密传输 + AutoTLS）

`acs-server` 拆分为**两个监听服务**，实现「后台管理不开放公网，仅开放 client/mirror API」：

| 服务 | 默认端口 | 绑定 | 暴露内容 |
|---|---|---|---|
| **公开 API** | **9600** | `0.0.0.0` | 仅 client / mirror 调用：`/api/client/*`、`/api/mirror/pull`（apikey 认证，无网页、无管理） |
| **后台管理** | **9680** | `127.0.0.1`（仅本机） | 网页后台 `/login` `/root` `/finance` + 管理 API `/api/admin/*`、`/api/accounts`、`/api/stats`、`/api/audit`、`/api/members`、`/api/admin/mirror-keys` |

> 对外（nginx 反代）只代理 **9600**；9680 管理端**不开放公网**，管理员在本机访问，
> 或经 SSH/RDP 隧道访问。

---

## 一、架构

```
公网客户端(https) ──► nginx(:443 TLS 终止) ──► acs-server 公开 API (:9600)
内网管理员(浏览器) ────────────────────────► acs-server 后台管理 (:9680，仅本机)
```

---

## 二、AutoTLS：自动生成证书（自签名）

```powershell
cd deploy/certs
powershell -ExecutionPolicy Bypass -File generate.ps1 -DnsName acs.aeu.org -InstallTrust
```

生成 `deploy/certs/cert.pem` + `key.pem`（无 OpenSSL 也可，PowerShell 5.1+ 即可）。
正式证书：acme.sh/certbot 申请后覆盖这两个文件（或改 nginx 路径）。

---

## 三、启动 acs-server（双端口）

```powershell
cd D:\Programs\Economy\ACSystem
cargo run -p acs-server
# 日志：
#   [acs-server] 公开 API（client/mirror）: http://0.0.0.0:9600
#   [acs-server] 后台管理（仅内网）: http://127.0.0.1:9680
```

可用环境变量覆盖：

| 变量 | 默认 | 说明 |
|---|---|---|
| `ACS_PUBLIC_PORT` | `9600` | 公开 API 端口 |
| `ACS_PUBLIC_BIND` | `0.0.0.0` | 公开 API 监听地址 |
| `ACS_ADMIN_PORT` | `9680` | 后台管理端口 |
| `ACS_ADMIN_BIND` | `127.0.0.1` | 后台管理监听地址（保持本机即不开放公网） |

---

## 四、安装 nginx for Windows（对外仅代理 9600）

1. 下载 <https://nginx.org/en/download.html>，解压到 `C:\nginx`。
2. 将 `deploy/nginx/nginx-acs.conf` 覆盖到 `C:\nginx\conf\nginx.conf`，
   修改 `server_name` 与证书路径。
3. 启动：
```powershell
C:\nginx\nginx.exe
C:\nginx\nginx.exe -s reload
```
> nginx 已把 `location /` 反代到 `127.0.0.1:9600`（公开 API）。
> 后台管理 9680 不经过 nginx，仅本机可达。

验证：浏览器 `https://acs.aeu.org/api/status`（若配置镜像服务）或
`https://acs.aeu.org/api/mirror/pull`（POST）应可达。

---

## 五、客户端配置（https，不跳过校验）

### acs-client（Alpha Wallet）
```powershell
# 创建时指定公开端口：
acs-client new --uid Steve --email Steve@aeu.org --pass 'xxx' --server https://acs.aeu.org --apikey mir-xxx
# 或运行期修改：
acs-client config --server https://acs.aeu.org --apikey mir-xxx
```
> 内网直连时用 `--server http://<server-ip>:9600`。

### acs-mirror
```powershell
acs-mirror config --server https://acs.aeu.org --apikey mir-xxx
acs-mirror sync
```

### 自签名证书信任（关键：**不跳过校验**）
client/mirror 使用 Windows 系统 TLS（schannel）校验证书链：
```powershell
certutil -addstore -f Root deploy/certs/cert.pem   # 需管理员
```
正式证书则无需导入（系统根证书自动信任）。

---

## 六、安全要点

- **管理端隔离**：9680 默认绑定 `127.0.0.1`，公网不可达；对外只暴露 9600 的
  client/mirror 端点（apikey 认证 + 交易 ed25519 签名校验）。
- nginx 只开 443（HTTPS），80 自动跳转。
- 后端防护：请求体 4MB、超时 30s、隐藏 Server 头、审计留痕。

---

## 七、常见问题

| 现象 | 处理 |
|---|---|
| client/mirror 报 403 | apikey 无效：在管理后台（9680）`/api/admin/mirror-keys` 重新生成 |
| 管理后台公网访问不到 | 正常：9680 仅本机；需要远程管理请用 SSH/RDP 隧道 |
| 浏览器提示"不安全" | 自签名证书未信任：`certutil -addstore -f Root cert.pem` 或换正式证书 |
| nginx 报证书路径错误 | 检查 nginx.conf 的 `ssl_certificate*` 路径 |

---

## 八、权能隔离与安全模型

两个端口在**路由暴露面**与**认证模型**上完全隔离（已实测验证）。

### 8.1 暴露面对照

| 能力 | 公开 9600 | 管理 9680 |
|---|---|---|
| 账户开立 / 提交 / 确认 / 拒绝 / 待确认（`/api/client/*`） | ✅ | ❌ 404 |
| 镜像拉取（`/api/mirror/pull`） | ✅ | ❌ 404 |
| 网页后台（`/login` `/root` `/finance`） | ❌ 404 | ✅ |
| 后台管理 API（`/api/admin/*`、accounts/stats/audit/members/keys/mirror-keys） | ❌ 404 | ✅ |

### 8.2 认证模型（互不通用）

| 端口 | 凭证 | 说明 |
|---|---|---|
| 9600 | **镜像 apikey**（`mirror_keys`，管理员在管理端创建） | 弱凭证：只读镜像本意；写操作（开立/提交/确认）另有 **ed25519 签名** 二次保护 |
| 9680 | **管理员 Bearer token**（登录颁发，10 分钟滑动过期） | 强凭证：可管理账户/铸造/充值/审计 |

两者凭证体系独立：apikey 无法访问管理端，admin token 在公开端也无对应路由。

### 8.3 共享状态为何安全

两服务共享同一 `AppState`（数据库、会话、审计解锁态、根密钥解锁态），但：
- 公开端**没有任何**依赖 admin token 的路由 → 共享会话表无风险；
- 公开端**没有任何**审计/铸造路由 → 共享审计解锁、根密钥解锁态无风险；
- 镜像拉取若根密钥已解锁（在管理端解锁），会附带**中心签名**，这是特性（快照可验真）而非漏洞。

### 8.4 威胁模型（apikey 泄露影响评估）

| 攻击者持有 apikey 可做 | 是否危险 | 原因 |
|---|---|---|
| 拉取只读镜像快照 | 低 | 本意即公开账本只读 |
| 开立账户（任意 UID，用自己的公钥） | 中 | 可能**占用他人 UID 名字**（抢注/DoS）；但无法冒用他人身份交易 |
| 提交交易 / 确认 / 拒绝 | 低 | 需对应账户的 **ed25519 私钥签名**，攻击者没有私钥无法伪造资金流 |

**建议**
- apikey 视为敏感凭证妥善保管（泄露后到 9680 停用并重建）；
- 管理端保持 `ACS_ADMIN_BIND=127.0.0.1`，远程管理走 SSH/RDP 隧道；
- 如担心 UID 抢注，可在公开端口前加一层访问控制，或将 `open` 改为管理审核制；
- 可选：在 nginx 层为 9600 增加速率限制（`limit_req`），防滥用。

### 8.5 防 CSRF / 信息泄露

- 管理网页使用 `localStorage` 的 Bearer token（非 Cookie），天然免疫 CSRF；
- 公开端只返回账户余额/交易等账本数据，不返回任何密码、私钥、apikey 明文。
