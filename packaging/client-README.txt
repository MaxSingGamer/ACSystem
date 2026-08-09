A€ (Alpha Coin) 钱包客户端 — acs-client
================================================

【用法】
  acs-client <子命令>     不带子命令进入 TUI 交互界面

【常用子命令】
  new     创建钱包（首次使用；参数可缺省，交互输入）
          --uid <名> --email <邮箱> --pass <口令> --server <中心> --apikey <key>
  status  查看钱包状态
  sync    从中心拉取一次
  open    在中心开立账户（上传公钥）
  send <UID> <金额> --pass <口令>    本地签名转账（写入 outbox）
  submit  提交 outbox 待确认交易
  confirm --pass <口令> [--tx_id x] [--reject 理由]   确认/拒绝
  config  --server <中心地址> --apikey <key>   修改中心配置

【钱包目录】
  默认 %USERPROFILE%\.alpha_dir，可用 ACS_ALPHA_DIR 覆盖。

【中心地址】
  内网: http://<ip>:9600
  公网: https://<穿透域名>（AutoTLS 证书自动受信）
