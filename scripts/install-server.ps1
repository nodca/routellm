param(
    [string]$Repo = $(if ($env:LLMROUTER_REPO) { $env:LLMROUTER_REPO } else { "nodca/routellm" }),
    [string]$Tag = $(if ($env:LLMROUTER_TAG) { $env:LLMROUTER_TAG } else { "latest" }),
    [string]$AssetUrl = $env:LLMROUTER_ASSET_URL,
    [string]$InstallDir = $(if ($env:LLMROUTER_INSTALL_DIR) { $env:LLMROUTER_INSTALL_DIR } else { (Join-Path $env:LOCALAPPDATA "llmrouter") }),
    [string]$Bind = $(if ($env:LLMROUTER_BIND_ADDR) { $env:LLMROUTER_BIND_ADDR } else { "0.0.0.0:1290" }),
    [string]$MasterKey = $env:LLMROUTER_MASTER_KEY,
    [string]$RequestTimeout = $(if ($env:LLMROUTER_REQUEST_TIMEOUT_SECS) { $env:LLMROUTER_REQUEST_TIMEOUT_SECS } else { "90" }),
    [string]$ConfigFile = "",
    [string]$StartupTaskName = $(if ($env:LLMROUTER_WINDOWS_TASK_NAME) { $env:LLMROUTER_WINDOWS_TASK_NAME } else { "llmrouter" }),
    [switch]$SkipAutostart,
    [switch]$SkipStart,
    [switch]$SkipRunScript
)

$ErrorActionPreference = "Stop"

function Get-ArchName {
    switch ($env:PROCESSOR_ARCHITECTURE) {
        "AMD64" { return "x86_64" }
        "ARM64" { return "aarch64" }
        default { throw "Unsupported Windows architecture: $($env:PROCESSOR_ARCHITECTURE)" }
    }
}

function Get-DownloadUrl([string]$AssetName) {
    if ($AssetUrl) { return $AssetUrl }
    if ($Tag -eq "latest") {
        return "https://github.com/$Repo/releases/latest/download/$AssetName"
    }
    return "https://github.com/$Repo/releases/download/$Tag/$AssetName"
}

function New-MasterKey {
    return "sk-llmrouter-" + ([guid]::NewGuid().ToString("N").Substring(0, 24))
}

function Test-IsAdmin {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Register-StartupTask([string]$TaskName, [string]$ScriptPath) {
    $action = New-ScheduledTaskAction `
        -Execute "powershell.exe" `
        -Argument "-NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$ScriptPath`""
    $trigger = New-ScheduledTaskTrigger -AtStartup
    $principal = New-ScheduledTaskPrincipal -UserId "SYSTEM" -LogonType ServiceAccount -RunLevel Highest
    $settings = New-ScheduledTaskSettingsSet `
        -AllowStartIfOnBatteries `
        -DontStopIfGoingOnBatteries `
        -MultipleInstances IgnoreNew `
        -StartWhenAvailable

    Register-ScheduledTask `
        -TaskName $TaskName `
        -Action $action `
        -Trigger $trigger `
        -Principal $principal `
        -Settings $settings `
        -Force | Out-Null
}

if (-not $ConfigFile) {
    $ConfigFile = Join-Path $InstallDir "llmrouter.toml"
}
if (-not $MasterKey) {
    $MasterKey = New-MasterKey
}

$AssetName = "llmrouter-windows-$(Get-ArchName).zip"
$TempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("llmrouter-server-" + [guid]::NewGuid().ToString("N"))
$ArchivePath = Join-Path $TempDir $AssetName
$DatabaseUrl = "sqlite://llmrouter-state.db"
$RunScript = Join-Path $InstallDir "run-llmrouter.ps1"
$EnvFile = Join-Path $InstallDir "server.env"

New-Item -ItemType Directory -Force -Path $TempDir | Out-Null
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

$Url = Get-DownloadUrl $AssetName
Write-Host "Downloading $Url"
Invoke-WebRequest -Uri $Url -OutFile $ArchivePath
Expand-Archive -LiteralPath $ArchivePath -DestinationPath $TempDir -Force

$PackageRoot = Get-ChildItem -Path $TempDir -Directory | Where-Object { $_.FullName -ne $TempDir } | Select-Object -First 1
if (-not $PackageRoot) {
    throw "Downloaded archive does not contain a package directory"
}

Copy-Item (Join-Path $PackageRoot.FullName "llmrouter.exe") (Join-Path $InstallDir "llmrouter.exe") -Force

if (-not (Test-Path $ConfigFile)) {
@"
[routing]
default_cooldown_seconds = 300

[routing.cooldowns]
auth_error = 1800
rate_limited = 45
upstream_server_error = 300
transport_error = 30
edge_blocked = 1800
upstream_path_error = 1800
unknown_error = 300

[routing.manual_intervention]
auth_error = true
upstream_path_error = true
"@ | Set-Content -Path $ConfigFile
}

if (-not $SkipRunScript) {
@"
Set-Location "$InstallDir"
`$env:LLMROUTER_BIND_ADDR = "$Bind"
`$env:LLMROUTER_DATABASE_URL = "$DatabaseUrl"
`$env:LLMROUTER_REQUEST_TIMEOUT_SECS = "$RequestTimeout"
`$env:LLMROUTER_MASTER_KEY = "$MasterKey"
`$env:LLMROUTER_CONFIG_PATH = "$ConfigFile"
& "$InstallDir\llmrouter.exe"
"@ | Set-Content -Path $RunScript
}

@"
LLMROUTER_BIND_ADDR=$Bind
LLMROUTER_DATABASE_URL=$DatabaseUrl
LLMROUTER_REQUEST_TIMEOUT_SECS=$RequestTimeout
LLMROUTER_MASTER_KEY=$MasterKey
LLMROUTER_CONFIG_PATH=$ConfigFile
"@ | Set-Content -Path $EnvFile -Encoding ASCII

$AutostartEnabled = $false
if (-not $SkipAutostart) {
    if (-not (Test-IsAdmin)) {
        throw "Windows server autostart requires an elevated PowerShell session. Re-run as Administrator, or pass -SkipAutostart."
    }

    Register-StartupTask -TaskName $StartupTaskName -ScriptPath $RunScript
    $AutostartEnabled = $true

    if (-not $SkipStart) {
        try {
            Start-ScheduledTask -TaskName $StartupTaskName
        } catch {
            Write-Warning "Autostart task was created, but the immediate start attempt failed: $($_.Exception.Message)"
        }
    }
}

Remove-Item $TempDir -Recurse -Force

Write-Host "Server installation complete."
Write-Host ""
Write-Host "Binary:"
Write-Host "  $(Join-Path $InstallDir 'llmrouter.exe')"
Write-Host "Config:"
Write-Host "  $ConfigFile"
Write-Host "Env file:"
Write-Host "  $EnvFile"
Write-Host "Master key:"
Write-Host "  $MasterKey"
if ($AutostartEnabled) {
    Write-Host "Startup task:"
    Write-Host "  $StartupTaskName"
    Write-Host "Autostart:"
    Write-Host "  enabled"
} else {
    Write-Host "Autostart:"
    Write-Host "  disabled"
}
if (-not $SkipRunScript) {
    Write-Host "Run script:"
    Write-Host "  $RunScript"
    Write-Host ""
    Write-Host "Run:"
    Write-Host "  powershell -ExecutionPolicy Bypass -File `"$RunScript`""
    Write-Host ""
    Write-Host "Tip:"
    Write-Host "  Override -InstallDir if you want a different location."
}
