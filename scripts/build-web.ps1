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

    # Stamp the crate version into web/llms.txt so the deployed bundle
    # advertises its freshness (curl llms.txt | head). Keeps it from
    # drifting from Cargo.toml without a manual bump step.
    $verMatch = Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
    if ($verMatch) {
        $ver = $verMatch.Matches.Groups[1].Value
        $line = '**version:** ' + $ver + ' (stamped from Cargo.toml by build-web; matches crates.io when the deployed bundle is current)'
        (Get-Content web/llms.txt -Raw) -replace '(?m)^\*\*version:\*\* .*$', $line |
            Set-Content web/llms.txt -Encoding utf8 -NoNewline
        Write-Host "stamped llms.txt version: $ver" -ForegroundColor Cyan
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
