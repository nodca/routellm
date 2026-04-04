param(
    [string]$InstallDir = $(if ($env:LLMROUTER_INSTALL_DIR) { $env:LLMROUTER_INSTALL_DIR } else { (Join-Path $env:LOCALAPPDATA "llmrouter") }),
    [string]$StartupTaskName = $(if ($env:LLMROUTER_WINDOWS_TASK_NAME) { $env:LLMROUTER_WINDOWS_TASK_NAME } else { "llmrouter" }),
    [switch]$KeepStartupTask,
    [switch]$KeepInstallDir
)

$ErrorActionPreference = "Stop"

$TuiConfigDir = Join-Path $env:LOCALAPPDATA "llmrouter"

if (-not $KeepStartupTask) {
    try {
        $task = Get-ScheduledTask -TaskName $StartupTaskName -ErrorAction Stop
        if ($task) {
            Unregister-ScheduledTask -TaskName $StartupTaskName -Confirm:$false
        }
    } catch {
    }
}

if (-not $KeepInstallDir -and (Test-Path $InstallDir)) {
    Remove-Item $InstallDir -Recurse -Force
}

if ($TuiConfigDir -ne $InstallDir -and (Test-Path $TuiConfigDir)) {
    Remove-Item $TuiConfigDir -Recurse -Force
}

Write-Host "Windows uninstall complete."
Write-Host ""
if (-not $KeepStartupTask) {
    Write-Host "Removed startup task:"
    Write-Host "  $StartupTaskName"
}
if (-not $KeepInstallDir) {
    Write-Host "Removed:"
    Write-Host "  $InstallDir"
} else {
    Write-Host "Install directory kept:"
    Write-Host "  $InstallDir"
}
