# scripts/verify-onchain.ps1 — Windows entry point for the opt-in TRUST-LAYER proof.
# Hits the LIVE testnet + spends the sponsor key's gas (a real mint) — NOT part of
# the default verify.ps1. The proof is cargo (see verify-onchain.sh); this just
# delegates to the bash version through git-bash so there is a single source of truth.
param([string]$Name = "")
& bash "$PSScriptRoot/verify-onchain.sh" $Name
exit $LASTEXITCODE
