param(
  [Parameter(Mandatory = $true)]
  [string]$Directory
)

$ErrorActionPreference = "Stop"
$root = (Resolve-Path -LiteralPath $Directory).Path
$output = Join-Path $root "SHA256SUMS.txt"
$lines = Get-ChildItem -LiteralPath $root -File |
  Where-Object { $_.Name -ne "SHA256SUMS.txt" } |
  Sort-Object Name |
  ForEach-Object {
    $hash = Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256
    "$($hash.Hash.ToLowerInvariant())  $($_.Name)"
  }
$lines | Set-Content -LiteralPath $output -Encoding ascii
