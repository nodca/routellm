param(
    [string]$Repo = $(if ($env:LLMROUTER_REPO) { $env:LLMROUTER_REPO } else { "nodca/routellm" }),
    [string]$Tag = $(if ($env:LLMROUTER_TAG) { $env:LLMROUTER_TAG } else { "latest" }),
    [string]$Bind = $(if ($env:LLMROUTER_BIND_ADDR) { $env:LLMROUTER_BIND_ADDR } else { "127.0.0.1:1290" }),
    [string]$MasterKey = $env:LLMROUTER_MASTER_KEY,
    [string]$InstallDir = $(if ($env:LLMROUTER_INSTALL_DIR) { $env:LLMROUTER_INSTALL_DIR } else { (Join-Path $env:LOCALAPPDATA "llmrouter") }),
    [string]$ConfigFile = "",
    [string]$StartupTaskName = $(if ($env:LLMROUTER_WINDOWS_TASK_NAME) { $env:LLMROUTER_WINDOWS_TASK_NAME } else { "llmrouter" }),
    [switch]$SkipStart
)

$ErrorActionPreference = "Stop"

function Get-RawScriptUrl([string]$ScriptName) {
    $ref = "main"
    if ($Tag -ne "latest") {
        $ref = $Tag
    }
    return "https://raw.githubusercontent.com/$Repo/$ref/scripts/$ScriptName"
}

$TuiConfigDir = Join-Path $env:LOCALAPPDATA "llmrouter"
$ServerEnvFile = Join-Path $InstallDir "server.env"

if (-not $ConfigFile) {
    $ConfigFile = Join-Path $InstallDir "llmrouter.toml"
}

$serverScript = [scriptblock]::Create((Invoke-RestMethod (Get-RawScriptUrl "install-server.ps1")))
$tuiScript = [scriptblock]::Create((Invoke-RestMethod (Get-RawScriptUrl "install-tui.ps1")))

& $serverScript `
    -Repo $Repo `
    -Tag $Tag `
    -InstallDir $InstallDir `
    -Bind $Bind `
    -MasterKey $MasterKey `
    -ConfigFile $ConfigFile `
    -StartupTaskName $StartupTaskName `
    -SkipStart:$SkipStart

if (-not $MasterKey) {
    if (Test-Path $ServerEnvFile) {
        $envValues = @{}
        foreach ($line in Get-Content $ServerEnvFile) {
            if ($line -match '^\s*([A-Z0-9_]+)=(.*)$') {
                $envValues[$matches[1]] = $matches[2]
            }
        }
        if ($envValues.ContainsKey("LLMROUTER_MASTER_KEY")) {
            $MasterKey = $envValues["LLMROUTER_MASTER_KEY"]
        }
    }
}

& $tuiScript `
    -Repo $Repo `
    -Tag $Tag `
    -InstallDir $InstallDir `
    -ConfigDir $TuiConfigDir `
    -Server "http://$Bind" `
    -AuthKey $MasterKey

Write-Host "Single-machine installation complete."
Write-Host ""
Write-Host "Server:"
Write-Host "  endpoint     http://$Bind"
Write-Host "  config       $ConfigFile"
Write-Host "  startup task $StartupTaskName"
Write-Host ""
Write-Host "TUI:"
Write-Host "  binary       $(Join-Path $InstallDir 'llmrouter-tui.exe')"
Write-Host "  alias        lrtui"
Write-Host "  config       $(Join-Path $TuiConfigDir 'tui.env')"
Write-Host ""
Write-Host "Management key:"
Write-Host "  $MasterKey"
Write-Host ""
Write-Host "Next steps:"
Write-Host "  1. Edit $ConfigFile and add your routes/channels."
Write-Host "  2. The server starts automatically at boot."
Write-Host "  3. Open a new terminal and run: lrtui"
