# ============================================================
#  AEU Alpha Coin System - AutoTLS certificate generator (Windows)
#
#  Generates a self-signed certificate (cert.pem) + private key
#  (key.pem, PKCS#1 RSA) into this folder for nginx TLS termination.
#  Works on PowerShell 5.1 WITHOUT OpenSSL (uses .NET + manual DER).
#  Optionally installs into the local Trusted Root so clients
#  verify over schannel WITHOUT skipping validation.
#
#  Usage:
#    powershell -ExecutionPolicy Bypass -File generate.ps1 [-DnsName acs.example.org] [-InstallTrust]
# ============================================================
param(
    [string]$DnsName = "localhost",
    [switch]$InstallTrust
)

$ErrorActionPreference = "Stop"
$certPem = Join-Path $PSScriptRoot "cert.pem"
$keyPem  = Join-Path $PSScriptRoot "key.pem"

Write-Host "== AutoTLS: generate self-signed cert (SAN=$DnsName) ==" -ForegroundColor Cyan

$openssl = Get-Command openssl -ErrorAction SilentlyContinue
if ($openssl) {
    Write-Host "OpenSSL found, using it..."
    & $openssl req -x509 -newkey rsa:2048 -nodes -keyout $keyPem -out $certPem -days 3650 -subj "/CN=$DnsName" -addext "subjectAltName=DNS:$DnsName" 2>$null
    if ($LASTEXITCODE -ne 0) {
        & $openssl req -x509 -newkey rsa:2048 -nodes -keyout $keyPem -out $certPem -days 3650 -subj "/CN=$DnsName"
    }
} else {
    Write-Host "Using .NET (PowerShell 5.1+ compatible)..."
    $cert = New-SelfSignedCertificate `
        -DnsName $DnsName `
        -CertStoreLocation "Cert:\CurrentUser\My" `
        -KeyExportPolicy Exportable `
        -KeyAlgorithm RSA -KeyLength 2048 `
        -NotAfter (Get-Date).AddYears(10) `
        -Provider "Microsoft Enhanced RSA and AES Cryptographic Provider"

    # public cert PEM
    $certB64 = [Convert]::ToBase64String($cert.Export([Security.Cryptography.X509Certificates.X509ContentType]::Cert), [Base64FormattingOptions]::InsertLineBreaks)
    "-----BEGIN CERTIFICATE-----`n$certB64`n-----END CERTIFICATE-----" | Set-Content $certPem -Encoding ascii

    # ---- DER helpers (return byte[], all concat wrapped in [byte[]]) ----
    function Get-IntBytes([byte[]]$b) {
        $i = 0
        while ($i -lt $b.Length - 1 -and $b[$i] -eq 0) { $i++ }
        if ($i -gt 0) { $b = [byte[]]$b[$i..($b.Length - 1)] }
        $body = $b
        if (($b[0] -band 0x80) -ne 0) { $body = [byte[]]( [byte[]](0) + $b ) }
        $len = $body.Length
        if ($len -lt 0x80) { return ,[byte[]]( [byte[]](0x02, [byte]$len) + $body ) }
        if ($len -lt 0x100) { return ,[byte[]]( [byte[]](0x02, 0x81, [byte]$len) + $body ) }
        return ,[byte[]]( [byte[]](0x02, 0x82, [byte]($len -shr 8), [byte]($len -band 0xFF)) + $body )
    }
    function Get-SeqBytes([byte[]]$c) {
        $len = $c.Length
        if ($len -lt 0x80) { return ,[byte[]]( [byte[]](0x30, [byte]$len) + $c ) }
        if ($len -lt 0x100) { return ,[byte[]]( [byte[]](0x30, 0x81, [byte]$len) + $c ) }
        return ,[byte[]]( [byte[]](0x30, 0x82, [byte]($len -shr 8), [byte]($len -band 0xFF)) + $c )
    }

    $rsa = [Security.Cryptography.X509Certificates.RSACertificateExtensions]::GetRSAPrivateKey($cert)
    $p = $rsa.ExportParameters($true)

    $i0 = Get-IntBytes ([byte[]](0))
    $in = Get-IntBytes $p.Modulus
    $ie = Get-IntBytes $p.Exponent
    $id = Get-IntBytes $p.D
    $ip = Get-IntBytes $p.P
    $iq = Get-IntBytes $p.Q
    $idp = Get-IntBytes $p.DP
    $idq = Get-IntBytes $p.DQ
    $iqinv = Get-IntBytes $p.InverseQ

    $body = [byte[]]($i0 + $in + $ie + $id + $ip + $iq + $idp + $idq + $iqinv)
    $rsaSeq = Get-SeqBytes $body   # RSAPrivateKey SEQUENCE (PKCS#1)

    $keyB64 = [Convert]::ToBase64String($rsaSeq, [Base64FormattingOptions]::InsertLineBreaks)
    "-----BEGIN RSA PRIVATE KEY-----`n$keyB64`n-----END RSA PRIVATE KEY-----" | Set-Content $keyPem -Encoding ascii
}

if (-not (Test-Path $certPem) -or -not (Test-Path $keyPem)) {
    Write-Host "Certificate generation FAILED" -ForegroundColor Red
    exit 1
}
Write-Host "Generated:" -ForegroundColor Green
Write-Host "  cert: $certPem"
Write-Host "  key : $keyPem"

if ($InstallTrust) {
    Write-Host "== Installing into Trusted Root (LocalMachine, admin required) ==" -ForegroundColor Cyan
    $certObj = Get-PfxCertificate -FilePath $certPem
    $store = New-Object Security.Cryptography.X509Certificates.X509Store("Root","LocalMachine")
    $store.Open("ReadWrite")
    $store.Add($certObj)
    $store.Close()
    Write-Host "Installed into Trusted Root. Clients can verify https://$DnsName normally." -ForegroundColor Green
    Write-Host "Note: other client machines must import cert.pem into their own Trusted Root." -ForegroundColor Yellow
}

Write-Host ""
Write-Host "== Next steps ==" -ForegroundColor Cyan
Write-Host "1) Point nginx at these certs (see deploy/nginx/nginx-acs.conf)"
Write-Host "2) Start acs-server (http://127.0.0.1:8080)"
Write-Host "3) Start nginx (https://$DnsName -> 8080)"
Write-Host "4) acs-client / acs-mirror use https://$DnsName (cert now trusted)"
