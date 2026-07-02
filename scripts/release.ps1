# scripts/release.ps1 — Windows entry point for the atomic release tool.
#
# Usage:
#   ./scripts/release.ps1 -Version 0.1.1
#
# This is a THIN WRAPPER that delegates to scripts/release.sh — the single
# source of truth for the release (pre-flight, version bump, verify, commit,
# tag, push, cargo publish, GH release). A hand-maintained PowerShell PORT of
# that logic silently drifted (it lacked release.sh's `cargo package --list`
# sanity step; release.sh lacked the port's `node` pre-flight) — the exact
# drift class that shipped a TESTNET bundle from build-web.ps1's old port.
# Delegating makes drift IMPOSSIBLE. Requires Git for Windows (bash.exe).
#
# Read RELEASING.md before using.

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string]$Version
)

$ErrorActionPreference = "Stop"
Push-Location (Join-Path $PSScriptRoot "..")
try {
    # Resolve Git Bash EXPLICITLY — do NOT trust `bash` on PATH, which is often
    # WSL's bash (a different environment with no cargo/gh/node toolchain).
    $bash = $null
    $candidates = @(
        (Join-Path $env:ProgramFiles 'Git\bin\bash.exe'),
        (Join-Path ${env:ProgramFiles(x86)} 'Git\bin\bash.exe'),
        (Join-Path $env:LOCALAPPDATA 'Programs\Git\bin\bash.exe')
    )
    foreach ($c in $candidates) {
        if ($c -and (Test-Path $c)) { $bash = $c; break }
    }
    if (-not $bash) {
        # Fall back to deriving it from git.exe's install dir (…\Git\cmd\git.exe -> …\Git\bin\bash.exe).
        $gitCmd = Get-Command git -ErrorAction SilentlyContinue
        if ($gitCmd) {
            $gitBash = Join-Path (Split-Path (Split-Path $gitCmd.Source)) 'bin\bash.exe'
            if (Test-Path $gitBash) { $bash = $gitBash }
        }
    }
    if (-not $bash) {
        Write-Error "Git Bash (bash.exe) not found. Install Git for Windows, or run 'bash scripts/release.sh $Version' directly."
    }

    Write-Host "-> delegating to scripts/release.sh via $bash" -ForegroundColor Cyan
    & $bash scripts/release.sh $Version
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
} finally {
    Pop-Location
}
