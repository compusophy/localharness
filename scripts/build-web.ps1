# Build the localharness browser-app wasm bundle into web/pkg/ for Vercel.
#
# Usage:
#   pwsh scripts/build-web.ps1   (or:  powershell -File scripts/build-web.ps1)
#
# This is a THIN WRAPPER that delegates to scripts/build-web.sh — the single
# source of truth for the build (gen-docs, gen-feedback-resolutions, RUSTFLAGS
# path-remapping for privacy, wasm-pack --features browser-app,mainnet, and the
# boot.js/icon cache-buster stamping). A hand-maintained PowerShell PORT of that
# logic silently drifted — it had dropped the `mainnet` feature (shipping a
# TESTNET bundle), the doc regeneration (stale docs), the path-remap (leaking the
# builder's username into the wasm), and the cache-buster (stale to returning
# visitors). Delegating makes drift IMPOSSIBLE. Requires Git for Windows (bash.exe).

$ErrorActionPreference = "Stop"
Push-Location $PSScriptRoot/..
try {
    # Resolve Git Bash EXPLICITLY — do NOT trust `bash` on PATH, which is often
    # WSL's bash (a different environment with no cargo/wasm-pack/node toolchain).
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
        Write-Error "Git Bash (bash.exe) not found. Install Git for Windows, or run 'bash scripts/build-web.sh' directly."
    }

    Write-Host "-> delegating to scripts/build-web.sh via $bash" -ForegroundColor Cyan
    & $bash scripts/build-web.sh
    if ($LASTEXITCODE -ne 0) { throw "build-web.sh failed (exit $LASTEXITCODE)" }
} finally {
    Pop-Location
}
