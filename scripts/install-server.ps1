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
        default { throw "暂不支持当前 Windows 架构：$($env:PROCESSOR_ARCHITECTURE)" }
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

function Get-EnvFileValue([string]$Path, [string]$Key) {
    if (-not (Test-Path $Path)) { return $null }
    foreach ($line in Get-Content $Path) {
        if ($line -match '^\s*([A-Z0-9_]+)=(.*)$' -and $matches[1] -eq $Key) {
            return $matches[2]
        }
    }
    return $null
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

$AssetName = "llmrouter-windows-$(Get-ArchName).zip"
$TempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("llmrouter-server-" + [guid]::NewGuid().ToString("N"))
$ArchivePath = Join-Path $TempDir $AssetName
$RunScript = Join-Path $InstallDir "run-llmrouter.ps1"
$EnvFile = Join-Path $InstallDir "server.env"

if (-not $ConfigFile) {
    $ConfigFile = Join-Path $InstallDir "llmrouter.toml"
}
if (-not $Bind) {
    $Bind = Get-EnvFileValue -Path $EnvFile -Key "LLMROUTER_BIND_ADDR"
}
if (-not $Bind) {
    $Bind = "0.0.0.0:1290"
}
if (-not $RequestTimeout) {
    $RequestTimeout = Get-EnvFileValue -Path $EnvFile -Key "LLMROUTER_REQUEST_TIMEOUT_SECS"
}
if (-not $RequestTimeout) {
    $RequestTimeout = "90"
}
if (-not $MasterKey) {
    $MasterKey = Get-EnvFileValue -Path $EnvFile -Key "LLMROUTER_MASTER_KEY"
}
if (-not $MasterKey) {
    $MasterKey = New-MasterKey
}
$DatabaseUrl = Get-EnvFileValue -Path $EnvFile -Key "LLMROUTER_DATABASE_URL"
if (-not $DatabaseUrl) {
    $DatabaseUrl = "sqlite://llmrouter-state.db"
}

New-Item -ItemType Directory -Force -Path $TempDir | Out-Null
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

$Url = Get-DownloadUrl $AssetName
Write-Host "正在下载：$Url"
Invoke-WebRequest -Uri $Url -OutFile $ArchivePath
Expand-Archive -LiteralPath $ArchivePath -DestinationPath $TempDir -Force

$PackageRoot = Get-ChildItem -Path $TempDir -Directory | Where-Object { $_.FullName -ne $TempDir } | Select-Object -First 1
if (-not $PackageRoot) {
    throw "下载的压缩包中未找到程序目录"
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
        throw "Windows 服务端自启动需要管理员权限。请用管理员 PowerShell 重新运行，或传入 -SkipAutostart。"
    }

    Register-StartupTask -TaskName $StartupTaskName -ScriptPath $RunScript
    $AutostartEnabled = $true

    if (-not $SkipStart) {
        try {
            Start-ScheduledTask -TaskName $StartupTaskName
        } catch {
            Write-Warning "已创建开机启动任务，但立即启动失败：$($_.Exception.Message)"
        }
    }
}

Remove-Item $TempDir -Recurse -Force

Write-Host "服务端安装完成。"
Write-Host ""
Write-Host "二进制文件："
Write-Host "  $(Join-Path $InstallDir 'llmrouter.exe')"
Write-Host "配置文件："
Write-Host "  $ConfigFile"
Write-Host "环境文件："
Write-Host "  $EnvFile"
Write-Host "管理 Key："
Write-Host "  $MasterKey"
if ($AutostartEnabled) {
    Write-Host "启动任务："
    Write-Host "  $StartupTaskName"
    Write-Host "开机自启："
    Write-Host "  已启用"
} else {
    Write-Host "开机自启："
    Write-Host "  未启用"
}
if (-not $SkipRunScript) {
    Write-Host "启动脚本："
    Write-Host "  $RunScript"
    Write-Host ""
    Write-Host "手动启动："
    Write-Host "  powershell -ExecutionPolicy Bypass -File `"$RunScript`""
    Write-Host ""
    Write-Host "提示："
    Write-Host "  如果你想换安装目录，可以传入 -InstallDir。"
}
