param(
    [string]$Repo = $(if ($env:LLMROUTER_REPO) { $env:LLMROUTER_REPO } else { "nodca/routellm" }),
    [string]$Tag = $(if ($env:LLMROUTER_TAG) { $env:LLMROUTER_TAG } else { "latest" }),
    [string]$AssetUrl = $env:LLMROUTER_ASSET_URL,
    [string]$InstallDir = $(if ($env:LLMROUTER_TUI_INSTALL_DIR) { $env:LLMROUTER_TUI_INSTALL_DIR } else { (Join-Path $env:LOCALAPPDATA "llmrouter") }),
    [string]$Server = $(if ($env:LLMROUTER_BASE_URL) { $env:LLMROUTER_BASE_URL } else { "http://127.0.0.1:1290" }),
    [string]$AuthKey = $env:LLMROUTER_AUTH_KEY,
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

$AssetName = "llmrouter-windows-$(Get-ArchName).zip"
$TempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("llmrouter-tui-" + [guid]::NewGuid().ToString("N"))
$ArchivePath = Join-Path $TempDir $AssetName
$RunScript = Join-Path $InstallDir "run-llmrouter-tui.ps1"

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

Copy-Item (Join-Path $PackageRoot.FullName "llmrouter-tui.exe") (Join-Path $InstallDir "llmrouter-tui.exe") -Force

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

Remove-Item $TempDir -Recurse -Force

Write-Host "TUI installation complete."
Write-Host ""
Write-Host "Binary:"
Write-Host "  $(Join-Path $InstallDir 'llmrouter-tui.exe')"
if (-not $SkipRunScript) {
    Write-Host "Run script:"
    Write-Host "  $RunScript"
    Write-Host ""
    Write-Host "Run:"
    Write-Host "  powershell -ExecutionPolicy Bypass -File `"$RunScript`""
}
