# Diff the upstream Python SDK against the commit we currently track.
#
# Usage:
#   ./scripts/sync-upstream.ps1                # diff vs. upstream HEAD
#   ./scripts/sync-upstream.ps1 -Ref <ref>     # diff vs. <ref>
#
# Does NOT modify the working tree. Prints commits, file stats, and
# suggested next steps.

[CmdletBinding()]
param(
    [string]$Ref = "HEAD"
)

$ErrorActionPreference = "Stop"
$UpstreamUrl = "https://github.com/google-antigravity/antigravity-sdk-python.git"

$pinnedLine = Select-String -Path "UPSTREAM.md" -Pattern '^\| Pinned commit' | Select-Object -First 1
if (-not $pinnedLine) {
    Write-Error "could not parse pinned commit from UPSTREAM.md"
    exit 1
}
$Pinned = ($pinnedLine.Line -split '`')[1]

$Scratch = Join-Path $env:TEMP ("localharness-sync-{0}" -f (Get-Random))
New-Item -ItemType Directory -Path $Scratch | Out-Null
try {
    Write-Host "==> cloning $UpstreamUrl"
    git clone --quiet --filter=blob:none $UpstreamUrl (Join-Path $Scratch "upstream") | Out-Null
    Push-Location (Join-Path $Scratch "upstream")
    try {
        git fetch --quiet origin $Pinned 2>$null
        $TargetSha = (git rev-parse $Ref).Trim()

        if ($Pinned -eq $TargetSha) {
            Write-Host "==> upstream unchanged; pinned commit matches $Ref ($TargetSha)"
            return
        }

        Write-Host ""
        Write-Host "==> commits in upstream since pinned ($Pinned..$TargetSha)"
        git log --oneline "$Pinned..$TargetSha"

        Write-Host ""
        Write-Host "==> files changed under google/antigravity/"
        git diff --stat "$Pinned..$TargetSha" -- google/antigravity/

        Write-Host ""
        Write-Host "==> changed file list"
        git diff --name-status "$Pinned..$TargetSha" -- google/antigravity/

        $short = $TargetSha.Substring(0, 8)
        Write-Host ""
        Write-Host "==> next steps"
        Write-Host "  1. Review the diff above."
        Write-Host "  2. Port the relevant changes into src/ of this repo."
        Write-Host "  3. Update the 'Pinned commit' line in UPSTREAM.md to $TargetSha."
        Write-Host "  4. Refresh the vendored snapshot at google/antigravity/ to match."
        Write-Host "  5. cargo test ; cargo clippy --all-targets ; cargo run --example smoke"
        Write-Host "  6. Commit: ""sync upstream $short"""
    }
    finally {
        Pop-Location
    }
}
finally {
    Remove-Item -Recurse -Force $Scratch -ErrorAction SilentlyContinue
}
