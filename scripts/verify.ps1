# scripts/verify.ps1 — Windows entry point for the proof-of-spec gate.
# The proofs are cargo + node (see verify.sh); this just delegates to the bash
# version through git-bash so there is a single source of truth.
param([string]$Cartridge = "bitmask.rl")
& bash "$PSScriptRoot/verify.sh" $Cartridge
exit $LASTEXITCODE
