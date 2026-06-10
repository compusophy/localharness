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

# PowerShell 5.1 wraps every line a native exe writes to stderr in an
# ErrorRecord. With $ErrorActionPreference = "Stop" that turns a cargo
# "Checking foo" progress line into a terminating error even though the
# process exited 0. Run native commands inside this wrapper so the EAP
# trap is scoped to their lifetime; we check $LASTEXITCODE ourselves.
function Invoke-Native([string]$Name, [scriptblock]$Block) {
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        & $Block
        if ($LASTEXITCODE -ne 0) { Fail "$Name failed (exit $LASTEXITCODE)" }
    }
    finally { $ErrorActionPreference = $prev }
}

# Validate version shape (X.Y.Z plus optional pre-release).
if ($Version -notmatch '^[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9.-]+)?$') {
    Fail "version must look like X.Y.Z (got '$Version')"
}

# ---------------------------------------------------------------------------
# Pre-flight
# ---------------------------------------------------------------------------

Step "pre-flight: tooling"
# node is required by the proof-of-spec gate (scripts/verify.sh runs the
# cartridge corpus proofs in node) — fail here, not mid-verify.
foreach ($cmd in @("cargo", "gh", "git", "node")) {
    if (-not (Get-Command $cmd -ErrorAction SilentlyContinue)) { Fail "$cmd not on PATH" }
}
# Resolve GIT-BASH explicitly: a bare `bash` on Windows PATH often resolves to
# the WSL launcher (System32\bash.exe), which dies with "no installed
# distributions" on machines without a WSL distro — exactly what aborted the
# first 0.31.0 attempt mid-gate. verify.sh needs git-bash, which ships beside
# git.exe (…\Git\cmd\git.exe → …\Git\bin\bash.exe).
$script:GitBash = Join-Path (Split-Path (Split-Path (Get-Command git).Source)) "bin\bash.exe"
if (-not (Test-Path $script:GitBash)) {
    Fail "git-bash not found at $script:GitBash (verify.sh needs git-bash, not WSL bash)"
}
try { gh auth status 2>&1 | Out-Null } catch { Fail "gh not authenticated (run: gh auth login)" }
if ($LASTEXITCODE -ne 0) { Fail "gh not authenticated (run: gh auth login)" }

Step "pre-flight: git state"
# CHANGELOG.md may be dirty — the user is staging release notes for
# *this* release. Every other dirty file is a hard error so we don't
# bundle unrelated work into the release commit.
$dirty = git status --porcelain | Where-Object { $_ -notmatch '^(\s|M)M CHANGELOG\.md$' }
if ($dirty) { Fail ("working tree has dirty files other than CHANGELOG.md:`n" + ($dirty -join "`n")) }
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

Step "stamp web/llms.txt version -> $Version"
# Keep the public llms.txt freshness line in lock-step with the release.
# Same method as scripts/build-web.ps1 so the two never fight over format.
$llmsLine = '**version:** ' + $Version + ' (stamped from Cargo.toml by build-web; matches crates.io when the deployed bundle is current)'
(Get-Content web/llms.txt -Raw) -replace '(?m)^\*\*version:\*\* .*$', $llmsLine |
    Set-Content web/llms.txt -Encoding utf8 -NoNewline
if (-not (Select-String -Path web/llms.txt -Pattern "^\*\*version:\*\* $([regex]::Escape($Version)) " -Quiet)) {
    Fail "llms.txt version stamp did not stick"
}

# ---------------------------------------------------------------------------
# Verify
# ---------------------------------------------------------------------------

Step "cargo build (refreshes Cargo.lock)"
Invoke-Native "cargo build" { cargo build --quiet }

Step "proof-of-spec gate (scripts/verify.sh)"
# Full end-to-end gate, mirroring release.sh: all feature-config tests (incl.
# anthropic + wallet), the wasm32 guardrail checks, and REAL cartridge
# instantiate/render/compose. Catches what a bare `cargo test` cannot — the
# browser app's wasm runtime never executes under the cargo suite. The gate
# runs the default-feature tests itself, so there is no separate `cargo test`
# step here.
Invoke-Native "scripts/verify.sh" { & $script:GitBash "$PSScriptRoot/verify.sh" }

Step "cargo clippy"
Invoke-Native "cargo clippy" { cargo clippy --all-targets -- -D warnings }

Step "cargo publish --dry-run"
Invoke-Native "cargo publish --dry-run" { cargo publish --dry-run --allow-dirty }

# ---------------------------------------------------------------------------
# Commit + tag + push
# ---------------------------------------------------------------------------

Step "git commit"
Invoke-Native "git add"    { git add Cargo.toml Cargo.lock CHANGELOG.md web/llms.txt }
Invoke-Native "git commit" { git commit -m "release $Tag" }

Step "git tag $Tag"
Invoke-Native "git tag" { git tag -a $Tag -m $Tag }

Step "git push --atomic origin main $Tag"
Invoke-Native "git push" { git push --atomic origin main $Tag }

# ---------------------------------------------------------------------------
# Publish + GH release
# ---------------------------------------------------------------------------

Step "cargo publish"
Invoke-Native "cargo publish" { cargo publish }

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
    Invoke-Native "gh release create" { gh release create $Tag --repo $Repo --title $Tag --notes-file $notesFile }
}
finally {
    Remove-Item $notesFile -ErrorAction SilentlyContinue
}

Step "done"
Write-Host ""
Write-Host "  crate:   https://crates.io/crates/localharness/$Version"
Write-Host "  release: https://github.com/$Repo/releases/tag/$Tag"
Write-Host ""
