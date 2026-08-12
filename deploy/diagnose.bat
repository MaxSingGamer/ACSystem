@echo off
setlocal enabledelayedexpansion
title AEU ACSystem - Connectivity Diagnostic
echo ============================================================
echo   AEU Alpha Coin - Connectivity Diagnostic
echo   Run on the server or any client machine.
echo ============================================================
echo.
set "HOST=acsystem.maxshin.top"
set "PORT=443"
set "LOCAL_PORT=9600"

echo [1/6] Local acs-server (port %LOCAL_PORT%) ...
powershell -NoProfile -Command "try{$t=New-Object Net.Sockets.TcpClient;$t.Connect('127.0.0.1',%LOCAL_PORT%);'  OK 127.0.0.1:%LOCAL_PORT% is listening';$t.Close()}catch{'  [FAIL] 127.0.0.1:%LOCAL_PORT% not listening - start acs-server.exe first'}"
echo.

echo [2/6] frp client process ...
tasklist | findstr /i "frpc.exe" >nul 2>&1
if %errorlevel%==0 (
  echo   OK frpc.exe is running
) else (
  echo   [WARN] frpc.exe not found - LoliaFRP client may not be started
)
echo.

echo [3/6] DNS for %HOST% ...
powershell -NoProfile -Command "try{$r=[Net.Dns]::GetHostAddresses('%HOST%'); foreach($a in $r){'  '+$a.IPAddressToString}; $p=$r|Where-Object{$_.AddressFamily -eq 'InterNetwork' -and $_.IPAddressToString -match '^(192\.168\.|10\.|172\.(1[6-9]|2[0-9]|3[01])\.)'}; if($p){'  [WARN] resolved to private/LAN IP - public tunnel may be unreachable'}}catch{'  [FAIL] cannot resolve'}"
echo   Public DNS 8.8.8.8:
nslookup %HOST% 8.8.8.8 2>nul | findstr /i "address" | findstr /v "127.0.0.1"
echo   hosts file entry:
findstr /i "%HOST%" "%WINDIR%\System32\drivers\etc\hosts" >nul 2>&1 && findstr /i "%HOST%" "%WINDIR%\System32\drivers\etc\hosts" || echo     (no entry)
echo.

echo [4/6] TCP %HOST%:%PORT% ...
powershell -NoProfile -Command "try{$t=New-Object Net.Sockets.TcpClient;$t.Connect('%HOST%',%PORT%);'  OK connected';$t.Close()}catch{'  [FAIL] cannot connect %HOST%:%PORT% - tunnel not bound or firewall'}"
echo.

echo [5/6] HTTPS /api/status ...
curl.exe --ssl-no-revoke -s -m 10 -w "\n  HTTP: %%{http_code}\n" https://%HOST%/api/status
echo.

echo [6/6] HTTPS /api/mirror/list ...
curl.exe --ssl-no-revoke -s -m 10 -o NUL -w "  HTTP: %%{http_code}\n" https://%HOST%/api/mirror/list
echo.
echo ============================================================
echo   How to read:
echo   - HTTP 200 + {"ok":true,...}  = public OK, client can sync
echo   - HTTP 403/404                = tunnel reachable but domain not bound / service issue
echo   - HTTP 000 / timeout          = domain not bound to tunnel, or frpc not running
echo   - Local 9600 not listening    = start acs-server.exe on the server first
echo   - frpc not found              = start LoliaFRP client and check tunnel config
echo ============================================================
echo.
pause
