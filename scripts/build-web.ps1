# Build the localharness browser-app wasm bundle into web/pkg/ for
# Vercel. The app code lives inside the main `localharness` crate
# behind the `browser-app` feature; wasm-pack drives it as a cdylib.
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

    Write-Host "→ wasm-pack build (release, browser-app)..." -ForegroundColor Cyan
    wasm-pack build . `
        --target web `
        --out-dir web/pkg `
        --release `
        --no-default-features `
        --features browser-app
    if ($LASTEXITCODE -ne 0) { throw "wasm-pack failed" }

    Write-Host "→ web/pkg/ updated. Commit the changes and push for Vercel to pick up." -ForegroundColor Green
    Get-ChildItem web/pkg | Select-Object Name, Length | Format-Table -AutoSize
} finally {
    Pop-Location
}
