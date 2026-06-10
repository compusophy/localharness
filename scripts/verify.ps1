# scripts/verify.ps1 — Windows entry point for the proof-of-spec gate.
# The proofs are cargo + node (see verify.sh); this just delegates to the bash
# version through git-bash so there is a single source of truth. GIT-BASH is
# resolved explicitly: a bare `bash` on Windows PATH often resolves to the WSL
# launcher (System32\bash.exe), which dies without an installed distro.
param([string]$Cartridge = "bitmask.rl")
$gitBash = Join-Path (Split-Path (Split-Path (Get-Command git).Source)) "bin\bash.exe"
if (-not (Test-Path $gitBash)) {
    Write-Error "git-bash not found at $gitBash (verify.sh needs git-bash, not WSL bash)"
    exit 1
}
& $gitBash "$PSScriptRoot/verify.sh" $Cartridge
exit $LASTEXITCODE
