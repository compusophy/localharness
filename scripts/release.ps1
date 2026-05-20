# scripts/release.ps1 — atomic release tool.
#
# Usage:
#   ./scripts/release.ps1 -Version 0.1.1
#
# Does the whole release in one go: pre-flight, version bump, verify,
# commit, tag, push, cargo publish, GH release. Each step exits on
# failure; the script never leaves a half-finished release.
#
# Read RELEASING.md before using.

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string]$Version
)

$ErrorActionPreference = "Stop"
$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
Set-Location $RepoRoot

$Tag   = "v$Version"
$Today = Get-Date -Format "yyyy-MM-dd"
$Repo  = "compusophy/localharness"

function Step($msg) { Write-Host "==> $msg" -ForegroundColor Green }
function Warn($msg) { Write-Host "!!  $msg" -ForegroundColor Yellow }
function Fail($msg) { Write-Host "xx  $msg" -ForegroundColor Red; exit 1 }

# Validate version shape (X.Y.Z plus optional pre-release).
if ($Version -notmatch '^[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9.-]+)?$') {
    Fail "version must look like X.Y.Z (got '$Version')"
}

# ---------------------------------------------------------------------------
# Pre-flight
# ---------------------------------------------------------------------------

Step "pre-flight: tooling"
foreach ($cmd in @("cargo", "gh", "git")) {
    if (-not (Get-Command $cmd -ErrorAction SilentlyContinue)) { Fail "$cmd not on PATH" }
}
try { gh auth status 2>&1 | Out-Null } catch { Fail "gh not authenticated (run: gh auth login)" }
if ($LASTEXITCODE -ne 0) { Fail "gh not authenticated (run: gh auth login)" }

Step "pre-flight: git state"
if ((git status --porcelain).Length -gt 0) { Fail "working tree dirty; commit/stash first" }
$branch = (git rev-parse --abbrev-ref HEAD).Trim()
if ($branch -ne "main") { Fail "not on main (on $branch)" }
git fetch --quiet origin main
$local  = (git rev-parse HEAD).Trim()
$remote = (git rev-parse origin/main).Trim()
$base   = (git merge-base HEAD origin/main).Trim()
if ($local -ne $remote -and $remote -ne $base) {
    Fail "local main diverges from origin/main; rebase first"
}

Step "pre-flight: tag availability"
$existsLocal = $false
try { git rev-parse $Tag 2>&1 | Out-Null; if ($LASTEXITCODE -eq 0) { $existsLocal = $true } } catch {}
if ($existsLocal) { Fail "tag $Tag already exists locally" }
$existsRemote = (git ls-remote --tags origin $Tag) -ne $null -and ((git ls-remote --tags origin $Tag).Length -gt 0)
if ($existsRemote) { Fail "tag $Tag already exists on origin" }

Step "pre-flight: CHANGELOG.md entry"
if (-not (Select-String -Path CHANGELOG.md -Pattern "^## \[$([regex]::Escape($Version))\]" -Quiet)) {
    Fail "CHANGELOG.md is missing a '## [$Version]' section (add it before releasing)"
}

# ---------------------------------------------------------------------------
# Bump
# ---------------------------------------------------------------------------

Step "bump Cargo.toml version -> $Version"
$cargoText = Get-Content -Raw -Encoding utf8 Cargo.toml
$pattern = '(?s)(\[package\][^\[]*?\r?\nversion = ")[^"]+(")'
$newText = [regex]::Replace($cargoText, $pattern, "`${1}$Version`${2}", 1)
if ($newText -eq $cargoText) { Fail "could not locate [package].version in Cargo.toml" }
# Write without BOM so cargo / git are happy.
[System.IO.File]::WriteAllText((Resolve-Path Cargo.toml), $newText, (New-Object System.Text.UTF8Encoding $false))

if (-not (Select-String -Path Cargo.toml -Pattern "^version = ""$([regex]::Escape($Version))""" -Quiet)) {
    Fail "Cargo.toml bump did not stick"
}

Step "promote CHANGELOG.md heading date -> $Today"
$clText = Get-Content -Raw -Encoding utf8 CHANGELOG.md
$clPattern = "(?m)^## \[$([regex]::Escape($Version))\][^\r\n]*"
$newCl = [regex]::Replace($clText, $clPattern, "## [$Version] - $Today", 1)
[System.IO.File]::WriteAllText((Resolve-Path CHANGELOG.md), $newCl, (New-Object System.Text.UTF8Encoding $false))

if (-not (Select-String -Path CHANGELOG.md -Pattern "^## \[$([regex]::Escape($Version))\] - $Today" -Quiet)) {
    Fail "CHANGELOG promote did not stick"
}

# ---------------------------------------------------------------------------
# Verify
# ---------------------------------------------------------------------------

Step "cargo build (refreshes Cargo.lock)"
cargo build --quiet; if ($LASTEXITCODE -ne 0) { Fail "cargo build failed" }

Step "cargo test"
cargo test --quiet; if ($LASTEXITCODE -ne 0) { Fail "cargo test failed" }

Step "cargo clippy"
cargo clippy --all-targets -- -D warnings; if ($LASTEXITCODE -ne 0) { Fail "cargo clippy failed" }

Step "cargo publish --dry-run"
cargo publish --dry-run --allow-dirty; if ($LASTEXITCODE -ne 0) { Fail "cargo publish dry-run failed" }

# ---------------------------------------------------------------------------
# Commit + tag + push
# ---------------------------------------------------------------------------

Step "git commit"
git add Cargo.toml Cargo.lock CHANGELOG.md
git commit -m "release $Tag" | Out-Null
if ($LASTEXITCODE -ne 0) { Fail "git commit failed" }

Step "git tag $Tag"
git tag -a $Tag -m $Tag
if ($LASTEXITCODE -ne 0) { Fail "git tag failed" }

Step "git push --atomic origin main $Tag"
git push --atomic origin main $Tag
if ($LASTEXITCODE -ne 0) { Fail "git push failed" }

# ---------------------------------------------------------------------------
# Publish + GH release
# ---------------------------------------------------------------------------

Step "cargo publish"
cargo publish
if ($LASTEXITCODE -ne 0) { Fail "cargo publish failed" }

Step "extract release notes from CHANGELOG.md"
$notesFile = [System.IO.Path]::GetTempFileName()
try {
    $clLines = Get-Content -Encoding utf8 CHANGELOG.md
    $inSection = $false
    $notes = New-Object System.Collections.Generic.List[string]
    foreach ($line in $clLines) {
        if ($line -match '^## \[') {
            if ($inSection) { break }
            if ($line -match "^## \[$([regex]::Escape($Version))\]") { $inSection = $true; continue }
        }
        if ($inSection) { $notes.Add($line) }
    }
    if ($notes.Count -eq 0) {
        Warn "release notes are empty; falling back to generic"
        $notes.Add("Release $Tag.")
    }
    [System.IO.File]::WriteAllText($notesFile, ($notes -join "`n"), (New-Object System.Text.UTF8Encoding $false))

    Step "gh release create $Tag"
    gh release create $Tag --repo $Repo --title $Tag --notes-file $notesFile
    if ($LASTEXITCODE -ne 0) { Fail "gh release create failed" }
}
finally {
    Remove-Item $notesFile -ErrorAction SilentlyContinue
}

Step "done"
Write-Host ""
Write-Host "  crate:   https://crates.io/crates/localharness/$Version"
Write-Host "  release: https://github.com/$Repo/releases/tag/$Tag"
Write-Host ""
