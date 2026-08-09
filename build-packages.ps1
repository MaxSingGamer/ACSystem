# build-packages.ps1 - build release binaries and pack one-click install zips
# Usage: powershell -ExecutionPolicy Bypass -File build-packages.ps1

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $root

Write-Host '[1/3] cargo build --release ...'
cargo build --release -p acs-server -p acs-client -p acs-mirror
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

$release = Join-Path $root 'target\release'
$dist = Join-Path $root 'dist'
if (Test-Path $dist) { Remove-Item $dist -Recurse -Force }
New-Item -ItemType Directory -Path $dist | Out-Null

function New-Package {
    param([string]$Name, [string]$Exe, [string]$Readme)
    $tmp = Join-Path $root ("dist\_tmp_" + $Name)
    if (Test-Path $tmp) { Remove-Item $tmp -Recurse -Force }
    New-Item -ItemType Directory -Path $tmp | Out-Null
    Copy-Item (Join-Path $release $Exe) $tmp -Force
    Copy-Item (Join-Path $root ("packaging\" + $Readme)) (Join-Path $tmp 'README.txt') -Force
    $zip = Join-Path $dist ($Name + '-windows-x64.zip')
    Compress-Archive -Path (Join-Path $tmp '*') -DestinationPath $zip -Force
    Remove-Item $tmp -Recurse -Force
    $kb = [math]::Round((Get-Item $zip).Length / 1KB)
    Write-Host ("   -> " + (Split-Path $zip -Leaf) + "  (" + $kb + " KB)")
}

Write-Host '[2/3] packaging ...'
New-Package -Name 'acs-server' -Exe 'acs-server.exe' -Readme 'server-README.txt'
New-Package -Name 'acs-client' -Exe 'acs-client.exe' -Readme 'client-README.txt'
New-Package -Name 'acs-mirror' -Exe 'acs-mirror.exe' -Readme 'mirror-README.txt'

Write-Host '[3/3] done. Output in dist/:'
Get-ChildItem $dist | Select-Object Name, @{n='Size(KB)';e={[math]::Round($_.Length/1KB)}} | Format-Table -AutoSize
