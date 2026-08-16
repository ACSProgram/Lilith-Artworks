param(
  [Parameter(Mandatory = $true)]
  [string]$ExpectedVersion
)

$ErrorActionPreference = "Stop"
$output = Join-Path (Get-Location) "release-assets"
if (Test-Path -LiteralPath $output) { throw "Release asset directory already exists: $output" }
New-Item -ItemType Directory -Path $output | Out-Null

$installers = @(Get-ChildItem -LiteralPath "src-tauri\target\release\bundle\nsis" -Filter "*-setup.exe" -File)
if ($installers.Count -ne 1) { throw "Expected exactly one NSIS installer, found $($installers.Count)." }
$installerName = "Lilith-Artworks-$ExpectedVersion-windows-x64-setup.exe"
$installer = Join-Path $output $installerName
Copy-Item -LiteralPath $installers[0].FullName -Destination $installer

$legalStage = Join-Path $env:RUNNER_TEMP "lilith-artworks-legal-$ExpectedVersion"
New-Item -ItemType Directory -Path (Join-Path $legalStage "models") -Force | Out-Null
Copy-Item -LiteralPath "LICENSE" -Destination (Join-Path $legalStage "LICENSE")
Copy-Item -LiteralPath "THIRD_PARTY_NOTICES.md" -Destination (Join-Path $legalStage "THIRD_PARTY_NOTICES.md")
Copy-Item -LiteralPath "licenses\THIRD_PARTY_LICENSES.html" -Destination (Join-Path $legalStage "THIRD_PARTY_LICENSES.html")
Copy-Item -LiteralPath "src-tauri\resources\models\LICENSE" -Destination (Join-Path $legalStage "models\LICENSE")
Compress-Archive -Path (Join-Path $legalStage "*") -DestinationPath (Join-Path $output "Lilith-Artworks-$ExpectedVersion-licenses.zip")

"installer=$installer" | Out-File -LiteralPath $env:GITHUB_OUTPUT -Append -Encoding utf8
