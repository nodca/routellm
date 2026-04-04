param(
    [string]$InstallDir = $(if ($env:LLMROUTER_INSTALL_DIR) { $env:LLMROUTER_INSTALL_DIR } else { (Join-Path $env:LOCALAPPDATA "llmrouter") }),
    [switch]$KeepInstallDir
)

$ErrorActionPreference = "Stop"

$TuiConfigDir = Join-Path $env:LOCALAPPDATA "llmrouter"

if (-not $KeepInstallDir -and (Test-Path $InstallDir)) {
    Remove-Item $InstallDir -Recurse -Force
}

if ($TuiConfigDir -ne $InstallDir -and (Test-Path $TuiConfigDir)) {
    Remove-Item $TuiConfigDir -Recurse -Force
}

Write-Host "Windows uninstall complete."
Write-Host ""
if (-not $KeepInstallDir) {
    Write-Host "Removed:"
    Write-Host "  $InstallDir"
} else {
    Write-Host "Install directory kept:"
    Write-Host "  $InstallDir"
}

