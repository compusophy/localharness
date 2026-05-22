# Build the localharness-web wasm bundle into web/pkg/ for Vercel.
#
# Usage:
#   pwsh scripts/build-web.ps1
#
# After running, commit the updated web/pkg/* artefacts and push —
# Vercel serves the static `web/` directory verbatim. The build is local
# (Vercel itself does no Rust compilation).

$ErrorActionPreference = "Stop"

Push-Location $PSScriptRoot/..
try {
    if (-not (Get-Command wasm-pack -ErrorAction SilentlyContinue)) {
        Write-Error "wasm-pack not on PATH. Install: cargo install wasm-pack"
    }

    Write-Host "→ wasm-pack build (release)..." -ForegroundColor Cyan
    Push-Location localharness-web
    try {
        wasm-pack build --target web --out-dir ../web/pkg --release
        if ($LASTEXITCODE -ne 0) { throw "wasm-pack failed" }
    } finally {
        Pop-Location
    }

    Write-Host "→ web/pkg/ updated. Commit the changes and push for Vercel to pick up." -ForegroundColor Green
    Get-ChildItem web/pkg | Select-Object Name, Length | Format-Table -AutoSize
} finally {
    Pop-Location
}
