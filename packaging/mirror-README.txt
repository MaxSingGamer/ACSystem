A€ (Alpha Coin) 只读镜像 — acs-mirror
================================================

【用法】
  config --server <中心地址> --apikey <key> [--central-pubkey <中心公钥>]
  sync            拉取中心增量账本与账户快照
  status          查看同步状态
  serve --port 9090   启动只读 HTTP 查询服务

【数据目录】
  默认 %USERPROFILE%\.alpha_mirror，可用 ACS_MIRROR_DIR 覆盖。

【apikey】
  镜像 apikey 在管理后台(9680) /api/admin/mirror-keys 生成。
