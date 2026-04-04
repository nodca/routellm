param(
    [string]$Repo = $(if ($env:LLMROUTER_REPO) { $env:LLMROUTER_REPO } else { "nodca/routellm" }),
    [string]$Tag = $(if ($env:LLMROUTER_TAG) { $env:LLMROUTER_TAG } else { "latest" }),
    [string]$AssetUrl = $env:LLMROUTER_ASSET_URL,
    [string]$InstallDir = $(if ($env:LLMROUTER_TUI_INSTALL_DIR) { $env:LLMROUTER_TUI_INSTALL_DIR } else { (Join-Path $env:LOCALAPPDATA "llmrouter") }),
    [string]$ConfigDir = $(if ($env:LLMROUTER_TUI_CONFIG_DIR) { $env:LLMROUTER_TUI_CONFIG_DIR } else { (Join-Path $env:LOCALAPPDATA "llmrouter") }),
    [string]$Server = $(if ($env:LLMROUTER_BASE_URL) { $env:LLMROUTER_BASE_URL } else { "http://127.0.0.1:1290" }),
    [string]$AuthKey = $env:LLMROUTER_AUTH_KEY,
    [switch]$SkipEnv,
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

function Add-UserPathEntry([string]$Entry) {
    $current = [Environment]::GetEnvironmentVariable("Path", "User")
    $parts = @()
    if ($current) {
        $parts = $current.Split(';') | Where-Object { $_ -and $_.Trim() -ne "" }
    }
    $normalizedEntry = [IO.Path]::GetFullPath($Entry).TrimEnd('\')
    $exists = $parts | Where-Object {
        [IO.Path]::GetFullPath($_).TrimEnd('\') -eq $normalizedEntry
    }
    if (-not $exists) {
        $updated = @($parts + $Entry) -join ';'
        [Environment]::SetEnvironmentVariable("Path", $updated, "User")
    }
    if (-not (($env:Path -split ';') | Where-Object { $_.TrimEnd('\') -eq $normalizedEntry })) {
        $env:Path = "$Entry;$env:Path"
    }
}

$AssetName = "llmrouter-windows-$(Get-ArchName).zip"
$TempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("llmrouter-tui-" + [guid]::NewGuid().ToString("N"))
$ArchivePath = Join-Path $TempDir $AssetName
$RunScript = Join-Path $InstallDir "run-llmrouter-tui.ps1"
$AliasCmd = Join-Path $InstallDir "lrtui.cmd"
$EnvFile = Join-Path $ConfigDir "tui.env"

New-Item -ItemType Directory -Force -Path $TempDir | Out-Null
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
New-Item -ItemType Directory -Force -Path $ConfigDir | Out-Null

$Url = Get-DownloadUrl $AssetName
Write-Host "正在下载：$Url"
Invoke-WebRequest -Uri $Url -OutFile $ArchivePath
Expand-Archive -LiteralPath $ArchivePath -DestinationPath $TempDir -Force

$PackageRoot = Get-ChildItem -Path $TempDir -Directory | Where-Object { $_.FullName -ne $TempDir } | Select-Object -First 1
if (-not $PackageRoot) {
    throw "下载的压缩包中未找到程序目录"
}

Copy-Item (Join-Path $PackageRoot.FullName "llmrouter-tui.exe") (Join-Path $InstallDir "llmrouter-tui.exe") -Force

@"
@echo off
"$InstallDir\llmrouter-tui.exe" %*
"@ | Set-Content -Path $AliasCmd -Encoding ASCII

if (-not $SkipEnv) {
    $lines = @("LLMROUTER_BASE_URL=$Server")
    if ($AuthKey) {
        $lines += "LLMROUTER_AUTH_KEY=$AuthKey"
    }
    Set-Content -Path $EnvFile -Value $lines -Encoding ASCII
}

if (-not $SkipRunScript) {
    $AuthLine = ""
    if ($AuthKey) {
        $AuthLine = "`$env:LLMROUTER_AUTH_KEY = `"$AuthKey`"`n"
    }
@"
`$env:LLMROUTER_BASE_URL = "$Server"
$AuthLine& "$InstallDir\llmrouter-tui.exe"
"@ | Set-Content -Path $RunScript
}

Add-UserPathEntry $InstallDir

Remove-Item $TempDir -Recurse -Force

Write-Host "TUI 安装完成。"
Write-Host ""
Write-Host "二进制文件："
Write-Host "  $(Join-Path $InstallDir 'llmrouter-tui.exe')"
Write-Host "快捷命令："
Write-Host "  $AliasCmd"
if (-not $SkipEnv) {
    Write-Host "配置文件："
    Write-Host "  $EnvFile"
}
if (-not $SkipRunScript) {
    Write-Host "启动脚本："
    Write-Host "  $RunScript"
    Write-Host ""
    Write-Host "启动方式："
    Write-Host "  powershell -ExecutionPolicy Bypass -File `"$RunScript`""
}
Write-Host ""
Write-Host "直接命令："
Write-Host "  lrtui"
