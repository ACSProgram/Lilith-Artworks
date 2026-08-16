param()

$ErrorActionPreference = "Stop"
$package = Get-Content -LiteralPath "package.json" -Raw | ConvertFrom-Json
$version = [string]$package.version
$isCandidate = $version -match "-rc\.\d+$"
$hasCertificate = -not [string]::IsNullOrWhiteSpace($env:WINDOWS_CERTIFICATE_BASE64)
$hasPassword = -not [string]::IsNullOrWhiteSpace($env:WINDOWS_CERTIFICATE_PASSWORD)
$hasTimestamp = -not [string]::IsNullOrWhiteSpace($env:WINDOWS_TIMESTAMP_URL)

if (($hasCertificate -or $hasPassword -or $hasTimestamp) -and -not ($hasCertificate -and $hasPassword -and $hasTimestamp)) {
  throw "Authenticode secrets are incomplete; certificate, password, and timestamp URL must be supplied together."
}
if (-not $isCandidate -and -not $hasCertificate) {
  throw "Stable releases require Authenticode certificate inputs."
}

"version=$version" | Out-File -LiteralPath $env:GITHUB_OUTPUT -Append -Encoding utf8
if (-not $hasCertificate) {
  "signed=false" | Out-File -LiteralPath $env:GITHUB_OUTPUT -Append -Encoding utf8
  exit 0
}

$pfxPath = Join-Path $env:RUNNER_TEMP "lilith-artworks-code-signing.pfx"
[IO.File]::WriteAllBytes($pfxPath, [Convert]::FromBase64String($env:WINDOWS_CERTIFICATE_BASE64))
$password = ConvertTo-SecureString -String $env:WINDOWS_CERTIFICATE_PASSWORD -Force -AsPlainText
$certificate = Import-PfxCertificate -FilePath $pfxPath -CertStoreLocation "Cert:\CurrentUser\My" -Password $password
if (-not $certificate.HasPrivateKey) { throw "Imported Authenticode certificate has no private key." }

$configPath = Join-Path $env:RUNNER_TEMP "tauri.release.conf.json"
@{
  bundle = @{
    windows = @{
      certificateThumbprint = $certificate.Thumbprint
      digestAlgorithm = "sha256"
      timestampUrl = $env:WINDOWS_TIMESTAMP_URL
    }
  }
} | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $configPath -Encoding utf8

"signed=true" | Out-File -LiteralPath $env:GITHUB_OUTPUT -Append -Encoding utf8
"config=$configPath" | Out-File -LiteralPath $env:GITHUB_OUTPUT -Append -Encoding utf8
