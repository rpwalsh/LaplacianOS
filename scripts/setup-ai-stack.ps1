#!/usr/bin/env pwsh

param(
    [string]$ScceRoot = "C:\Users\react\scce",
    [string]$ScceHost = "10.0.2.2",
    [int]$SccePort = 3000,
    [switch]$LaunchScce
)

$ErrorActionPreference = "Stop"

function Require-Command {
    param([Parameter(Mandatory = $true)][string]$Name)

    $cmd = Get-Command $Name -ErrorAction SilentlyContinue
    if (-not $cmd) {
        throw "Required command not found in PATH: $Name"
    }
    $cmd.Source
}

function Start-DetachedPowerShell {
    param(
        [Parameter(Mandatory = $true)][string]$WorkingDirectory,
        [Parameter(Mandatory = $true)][string]$Command
    )

    $escapedDir = $WorkingDirectory.Replace("'", "''")
    Start-Process -FilePath "powershell.exe" `
        -ArgumentList "-NoExit", "-Command", "Set-Location '$escapedDir'; $Command" `
        -WindowStyle Normal | Out-Null
}

$workspace = Split-Path $PSScriptRoot -Parent
$configDir = Join-Path $workspace "assets\config"
$configPath = Join-Path $configDir "modeld.json"

New-Item -ItemType Directory -Path $configDir -Force | Out-Null

if ($LaunchScce) {
    if (-not (Test-Path $ScceRoot)) {
        throw "SCCE root does not exist: $ScceRoot"
    }

    Write-Host "[ai] Launching SCCE server from $ScceRoot ..." -ForegroundColor Yellow
    Start-DetachedPowerShell -WorkingDirectory $ScceRoot -Command "npm run dev:server"
}

$existing = $null
if (Test-Path $configPath) {
    $existing = Get-Content -Path $configPath -Raw | ConvertFrom-Json
}

$config = [ordered]@{
    scce_host = $ScceHost
    scce_port = $SccePort
    doctrine = if ($null -ne $existing -and $null -ne $existing.doctrine) {
        [string]$existing.doctrine
    } else {
@"
SCCE is the graph-first synthesis stack backed by the local Walsh Technical Group codebase.
CastleHale demos are the proof deck for PowerWalk / WTG predictive math.
heterogeneousTemporalWalkEmbeddings.zip is the canonical temporal walk corpus for the predictive classifier lane.
Walsh-Hadamard transforms are separate from the Walsh Technical Group predictive classifier math.
Self-healing means graph-observed context, predictive hints, bounded recovery, and provenance-first operator loops.
"@
    }
}

$config | ConvertTo-Json -Depth 4 | Set-Content -Path $configPath -Encoding UTF8

Write-Host "[ai] Updated modeld provider config: $configPath" -ForegroundColor Green
Write-Host "[ai] Provider: SCCE (no inference fallback)" -ForegroundColor Green
Write-Host "[ai] SCCE:    $ScceHost`:$SccePort" -ForegroundColor Green
Write-Host "[ai] Host config written to $configPath" -ForegroundColor Cyan
Write-Host "[ai] After the next LaplacianOS build, the guest will see that file as /pkg/config/modeld.json inside the LaplacianOS VFS." -ForegroundColor Cyan
