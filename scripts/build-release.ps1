param(
    [string]$Tag = "",
    [string]$OutputDir = "dist"
)

$ErrorActionPreference = "Stop"

function Get-ArchName {
    switch ($env:PROCESSOR_ARCHITECTURE) {
        "AMD64" { return "x86_64" }
        "ARM64" { return "aarch64" }
        default { throw "Unsupported Windows architecture: $($env:PROCESSOR_ARCHITECTURE)" }
    }
}

$RootDir = Split-Path -Parent $PSScriptRoot
$Arch = Get-ArchName
$AssetBaseName = "metapi-windows-$Arch"
$DistDir = Join-Path $RootDir $OutputDir
$PackageDir = Join-Path ([System.IO.Path]::GetTempPath()) ("$AssetBaseName-" + [System.Guid]::NewGuid().ToString("N"))
$PackageRoot = Join-Path $PackageDir $AssetBaseName

New-Item -ItemType Directory -Force -Path $DistDir | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $PackageRoot "examples") | Out-Null

Push-Location $RootDir
cargo build --release --bin metapi-rs --bin metapi-tui
Pop-Location

Copy-Item (Join-Path $RootDir "target\release\metapi-rs.exe") (Join-Path $PackageRoot "metapi-rs.exe")
Copy-Item (Join-Path $RootDir "target\release\metapi-tui.exe") (Join-Path $PackageRoot "metapi-tui.exe")
Copy-Item (Join-Path $RootDir "examples\metapi.toml") (Join-Path $PackageRoot "examples\metapi.toml")
Copy-Item (Join-Path $RootDir "README.md") (Join-Path $PackageRoot "README.md")

$ArchivePath = Join-Path $DistDir "$AssetBaseName.zip"
if (Test-Path $ArchivePath) {
    Remove-Item $ArchivePath -Force
}
Compress-Archive -Path (Join-Path $PackageDir $AssetBaseName) -DestinationPath $ArchivePath

$Hash = Get-FileHash -Path $ArchivePath -Algorithm SHA256
"{0} *{1}" -f $Hash.Hash.ToLowerInvariant(), [System.IO.Path]::GetFileName($ArchivePath) | Set-Content (Join-Path $DistDir "SHA256SUMS")

Remove-Item $PackageDir -Recurse -Force

Write-Host "Built release asset:"
Write-Host "  $ArchivePath"
Write-Host ""
Write-Host "Release tag:"
if ([string]::IsNullOrWhiteSpace($Tag)) {
    Write-Host "  <not set>"
} else {
    Write-Host "  $Tag"
}
Write-Host ""
Write-Host "Checksums:"
Write-Host "  $(Join-Path $DistDir 'SHA256SUMS')"
