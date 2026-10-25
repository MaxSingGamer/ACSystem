# A€ QT 客户端构建/打包脚本（v3.3.0）
#
# 用法：
#   pwsh -File client\qt\build.ps1                 # 构建 Debug（开发）
#   pwsh -File client\qt\build.ps1 -Release        # 构建 Release 并打安装包
#
# 自动完成：cargo 构建 Rust DLL → CMake 构建两个 QT 客户端 → windeployqt 收集 Qt 运行库
#          →（-Release 时）Inno Setup 打安装包到 dist\。
param(
    [switch]$Release,
    [string]$QtDir = 'D:/Applications/Qt/6.12.0/msvc2022_64',
    [string]$CMake = 'D:/Applications/Qt/Tools/CMake_64/bin/cmake.exe',
    [string]$VcVars = 'C:\Program Files\Microsoft Visual Studio\18\Community\VC\Auxiliary\Build\vcvars64.bat',
    [string]$Iscc = 'C:\Program Files\Inno Setup 6\ISCC.exe'
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$cfg = if ($Release) { 'release' } else { 'debug' }
$qtCfg = if ($Release) { 'Release' } else { 'Debug' }
$buildDir = Join-Path $root "build\qt-$cfg"
$outDir = Join-Path $root "dist\qt-app"
$dll = Join-Path $root "target\$cfg\acs_client_core.dll"

Write-Host "== 1/4 Rust DLL ($cfg) ==" -ForegroundColor Cyan
Push-Location $root
try {
    if ($Release) { cargo build --release -p acs-client-core } else { cargo build -p acs-client-core }
    if ($LASTEXITCODE -ne 0) { throw "cargo 构建失败" }
} finally { Pop-Location }
if (-not (Test-Path $dll)) { throw "未找到 $dll" }

Write-Host "== 2/4 CMake 构建 QT 客户端 ($qtCfg) ==" -ForegroundColor Cyan
$batch = @"
@echo off
call "$VcVars" >nul
cd /d $root
"$CMake" -S client\qt -B "$buildDir" -G Ninja -DCMAKE_BUILD_TYPE=$qtCfg -DCMAKE_PREFIX_PATH="$QtDir" -DACS_CLIENT_CORE_DLL="$($dll -replace '\\','/')" >nul
if errorlevel 1 exit /b 1
"$CMake" --build "$buildDir"
if errorlevel 1 exit /b 1
"$CMake" -E make_directory "$outDir"
"$CMake" -E copy_if_different "$(if ($Release) { "build\qt-$cfg\bin\AlphaWallet.exe" } else { "build\qt-$cfg\bin\AlphaWallet.exe" })" "$outDir"
"$CMake" -E copy_if_different "$(if ($Release) { "build\qt-$cfg\bin\AlphaBankmanage.exe" } else { "build\qt-$cfg\bin\AlphaBankmanage.exe" })" "$outDir"
"$CMake" -E copy_if_different "$dll" "$outDir"
"$CMake" -E copy_if_different "$root\client\qt\README.txt" "$outDir"
echo DEPLOY_DONE
"@
$tmp = Join-Path $env:TEMP 'acs_qt_deploy.bat'
Set-Content $tmp $batch -Encoding ASCII
& cmd /c $tmp
if ($LASTEXITCODE -ne 0) { throw "CMake 构建/拷贝失败" }

Write-Host "== 3/4 windeployqt 收集 Qt 运行库 ==" -ForegroundColor Cyan
$wdq = Join-Path $QtDir 'bin\windeployqt.exe'
foreach ($exe in @('AlphaWallet.exe', 'AlphaBankmanage.exe')) {
    & $wdq --no-translations --no-system-d3d-compiler --no-opengl-sw (Join-Path $outDir $exe)
    if ($LASTEXITCODE -ne 0) { throw "windeployqt 失败：$exe" }
}

if (-not $Release) {
    Write-Host "`nDebug 构建完成：$outDir" -ForegroundColor Green
    return
}

Write-Host "== 4/4 Inno Setup 打安装包 ==" -ForegroundColor Cyan
$iss = Join-Path $root 'packaging\client-qt-setup.iss'
& $Iscc $iss
if ($LASTEXITCODE -ne 0) { throw "ISCC 失败" }
Write-Host "`n安装包完成：see dist\" -ForegroundColor Green
Get-ChildItem (Join-Path $root 'dist') -Filter '*Client*' | Select-Object Name, @{n = 'MB'; e = { [math]::Round($_.Length / 1MB, 1) } }
Write-Host "程序目录：$outDir"
