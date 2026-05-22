# Reproduce the exact POST our wasm makes, against Gemini directly.
# Isolates whether the bug is in the request shape (here) or in our
# wasm-side response parsing (api.rs SSE decoder).
#
# Usage:
#   pwsh scripts/probe-gemini.ps1            # non-streaming
#   pwsh scripts/probe-gemini.ps1 -Stream    # streaming SSE
#
# Reads GEMINI_API_KEY from .env in the repo root (gitignored).

param(
    [switch]$Stream
)

$ErrorActionPreference = "Stop"

$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$envFile = Join-Path $root ".env"

if (-not (Test-Path $envFile)) {
    Write-Error ".env not found at $envFile - copy .env.example and add your key."
}

$key = $null
Get-Content $envFile | ForEach-Object {
    if ($_ -match '^\s*GEMINI_API_KEY\s*=\s*(.+?)\s*$') {
        $key = $Matches[1].Trim('"').Trim("'")
    }
}
if (-not $key) { Write-Error "GEMINI_API_KEY missing from .env." }

$model = "gemini-3.5-flash"
$base = "https://generativelanguage.googleapis.com"

$body = @{
    contents = @(
        @{
            role  = "user"
            parts = @(@{ text = "Write one sentence about why Rust is good for agent SDKs." })
        }
    )
} | ConvertTo-Json -Depth 10 -Compress

Write-Host "-> body: $body" -ForegroundColor DarkGray

if ($Stream) {
    $url = "$base/v1beta/models/${model}:streamGenerateContent?alt=sse"
    Write-Host "-> POST $url" -ForegroundColor Cyan
    # Invoke-WebRequest with `-OutFile` is the cleanest way to see raw SSE.
    $tmp = New-TemporaryFile
    try {
        Invoke-WebRequest -Method Post -Uri $url `
            -Headers @{ "x-goog-api-key" = $key; "accept" = "text/event-stream" } `
            -ContentType "application/json" `
            -Body $body -OutFile $tmp.FullName -UseBasicParsing
        Write-Host "--- raw response body ---" -ForegroundColor Yellow
        Get-Content $tmp.FullName -Raw
    } finally {
        Remove-Item $tmp.FullName -Force -ErrorAction SilentlyContinue
    }
} else {
    $url = "$base/v1beta/models/${model}:generateContent"
    Write-Host "-> POST $url" -ForegroundColor Cyan
    try {
        $resp = Invoke-RestMethod -Method Post -Uri $url `
            -Headers @{ "x-goog-api-key" = $key } `
            -ContentType "application/json" `
            -Body $body
        Write-Host "--- response JSON ---" -ForegroundColor Yellow
        $resp | ConvertTo-Json -Depth 10
    } catch {
        Write-Host "--- error ---" -ForegroundColor Red
        Write-Host $_.Exception.Message
        if ($_.ErrorDetails) { Write-Host $_.ErrorDetails.Message }
    }
}
