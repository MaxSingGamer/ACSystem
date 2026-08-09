A€ (Alpha Coin) 中心服务器 — acs-server
================================================

【运行】
  双击或命令行运行 acs-server.exe。

【数据目录】
  默认 %TEMP%\acs-server-data，可用环境变量 ACS_DATA_DIR 覆盖。
  首次启动自动：迁移旧库 -> 种子默认根管理员 -> 种子系统账户
  (PreIssuedAccount / AESystem / AlphaEU，导出私钥到 ./alpha_dir)。

【两个监听服务】
  公开 API (client/mirror) : 默认 0.0.0.0:9600
      可用 ACS_PUBLIC_PORT / ACS_PUBLIC_BIND 覆盖
  后台管理 (网页)          : 默认 127.0.0.1:9680（仅本机，勿暴露公网）
      可用 ACS_ADMIN_PORT / ACS_ADMIN_BIND 覆盖

【管理后台登录】
  默认密码从环境变量 ACS_ADMIN_PASSWORD 读取；
  未设置则随机生成 16 位强密码并打印在启动日志中（登录后强制改密）。

【公网访问（可选）】
  用内网穿透（如 frp）把 9600 暴露到公网，HTTPS 由穿透服务商 AutoTLS 提供。
  9680 管理端请勿穿透，保持仅本机可访问。

【GnuPG】
  程序按 PATH -> 程序目录 -> 自动下载 Gpg4win 的顺序查找 gpg.exe。
