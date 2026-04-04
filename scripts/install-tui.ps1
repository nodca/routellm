param(
    [string]$Repo = $env:METAPI_REPO,
    [string]$Tag = $(if ($env:METAPI_TAG) { $env:METAPI_TAG } else { "latest" }),
    [string]$AssetUrl = $env:METAPI_ASSET_URL,
    [string]$InstallDir = $(if ($env:METAPI_TUI_INSTALL_DIR) { $env:METAPI_TUI_INSTALL_DIR } else { (Join-Path $env:LOCALAPPDATA "metapi") }),
    [string]$Server = $(if ($env:METAPI_BASE_URL) { $env:METAPI_BASE_URL } else { "http://127.0.0.1:8080" }),
    [string]$AuthKey = $env:METAPI_AUTH_KEY,
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
    if (-not $Repo) {
        throw "--Repo is required unless --AssetUrl is provided"
    }
    if ($Tag -eq "latest") {
        return "https://github.com/$Repo/releases/latest/download/$AssetName"
    }
    return "https://github.com/$Repo/releases/download/$Tag/$AssetName"
}

$AssetName = "metapi-windows-$(Get-ArchName).zip"
$TempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("metapi-tui-" + [guid]::NewGuid().ToString("N"))
$ArchivePath = Join-Path $TempDir $AssetName
$RunScript = Join-Path $InstallDir "run-metapi-tui.ps1"

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

Copy-Item (Join-Path $PackageRoot.FullName "metapi-tui.exe") (Join-Path $InstallDir "metapi-tui.exe") -Force

if (-not $SkipRunScript) {
    $AuthLine = ""
    if ($AuthKey) {
        $AuthLine = "`$env:METAPI_AUTH_KEY = `"$AuthKey`"`n"
    }
@"
`$env:METAPI_BASE_URL = "$Server"
$AuthLine& "$InstallDir\metapi-tui.exe"
"@ | Set-Content -Path $RunScript
}

Remove-Item $TempDir -Recurse -Force

Write-Host "TUI installation complete."
Write-Host ""
Write-Host "Binary:"
Write-Host "  $(Join-Path $InstallDir 'metapi-tui.exe')"
if (-not $SkipRunScript) {
    Write-Host "Run script:"
    Write-Host "  $RunScript"
    Write-Host ""
    Write-Host "Run:"
    Write-Host "  powershell -ExecutionPolicy Bypass -File `"$RunScript`""
}
