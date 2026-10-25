Alpha Wallet / AlphaBankmanage（A€ 客户端 v3.3.0）
=====================================================

两个客户端（安装后同目录，均从开始菜单启动）：

  AlphaWallet.exe      个人 / 国家 / 系统账户
  AlphaBankmanage.exe  企业 / 银行账户

首次启动：
  1. 选择中心服务（历史按「服务名称」区分；没有就点「新添」填中心地址）
  2. 选择账户类型并登录；没有账户可在登录页点「注册新账户」

数据目录（固定，不可改）：
  %USERPROFILE%\.alphawallet\        个人版
  %USERPROFILE%\.alphabankmanage\    企业版
  每个中心一个子目录（域名[_端口]），内含 wallet.db / gnupg / logs。
  中心历史：<上述目录>\central_server_history.db

技术支持：请提供 <数据目录>\logs 下的日志文件（已自动打码口令与用户名）。
