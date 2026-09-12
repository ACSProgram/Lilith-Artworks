param(
  [Parameter(Mandatory = $true)]
  [string]$InstallerPath,
  [Parameter(Mandatory = $true)]
  [string]$ExpectedVersion,
  [Parameter(Mandatory = $true)]
  [string]$InstallationDirectory,
  [switch]$RequireSignature
)

$ErrorActionPreference = "Stop"
$installer = Get-Item -LiteralPath $InstallerPath
if ($RequireSignature -and (Get-AuthenticodeSignature -LiteralPath $installer.FullName).Status -ne "Valid") {
  throw "NSIS installer Authenticode signature is not valid."
}
if (Test-Path -LiteralPath $InstallationDirectory) { throw "Installation test directory already exists." }

$process = Start-Process -FilePath $installer.FullName -ArgumentList "/S", "/D=$InstallationDirectory" -Wait -PassThru
if ($process.ExitCode -ne 0) { throw "Silent NSIS installation failed with exit code $($process.ExitCode)." }

$application = Get-ChildItem -LiteralPath $InstallationDirectory -Recurse -Filter "lilith-artworks.exe" -File | Select-Object -First 1
if (-not $application) { throw "Installed application executable is missing." }
$productVersion = [string]$application.VersionInfo.ProductVersion
if ($productVersion -notlike "$ExpectedVersion*") {
  throw "Installed executable version '$productVersion' does not match '$ExpectedVersion'."
}
if ($RequireSignature -and (Get-AuthenticodeSignature -LiteralPath $application.FullName).Status -ne "Valid") {
  throw "Installed application Authenticode signature is not valid."
}

$required = @(
  @{ Name = "LICENSE"; Pattern = "GNU GENERAL PUBLIC LICENSE" },
  @{ Name = "THIRD_PARTY_NOTICES.md"; Pattern = "Third-Party Notices" },
  @{ Name = "THIRD_PARTY_LICENSES.html"; Pattern = "Lilith Artworks $ExpectedVersion third-party licenses" }
)
foreach ($item in $required) {
  $file = Get-ChildItem -LiteralPath $InstallationDirectory -Recurse -Filter $item.Name -File | Select-Object -First 1
  if (-not $file) { throw "Installed legal resource is missing: $($item.Name)" }
  if ((Get-Content -LiteralPath $file.FullName -Raw) -notmatch [regex]::Escape($item.Pattern)) {
    throw "Installed legal resource has unexpected content: $($item.Name)"
  }
}
$modelLicenses = @(Get-ChildItem -LiteralPath $InstallationDirectory -Recurse -Filter "LICENSE" -File | Where-Object {
  (Get-Content -LiteralPath $_.FullName -Raw) -match "Copyright Adobe"
})
if ($modelLicenses.Count -eq 0) { throw "Adobe TrustMark model license is missing from the installed package." }

Write-Output "Installed NSIS package verified: $ExpectedVersion"
