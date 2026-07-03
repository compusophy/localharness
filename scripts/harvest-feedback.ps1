# THIN DELEGATING SHIM over scripts/check-feedback.mjs (the build-web.ps1
# pattern: logic lives in ONE maintained script). This script used to read the
# FeedbackFacet itself via `cast`, but it stayed pinned to the TESTNET diamond
# after the mainnet migration (stale chain) and required foundry.
# check-feedback.mjs is the replacement: node-only, reads BOTH chains, and
# knows the per-chain resolved-index files.
#
# Usage: pwsh scripts/harvest-feedback.ps1 [-Unresolved]
#   -Unresolved  hide resolved indices (mapped to check-feedback's --open)
# The old DIAMOND/RPC/RESOLVED env overrides no longer apply.

param([switch]$Unresolved)

$ErrorActionPreference = "Stop"

$mjs = Join-Path $PSScriptRoot 'check-feedback.mjs'
$extra = @()
if ($Unresolved) { $extra += '--open' }
node $mjs @extra
if ($LASTEXITCODE -ne 0) { throw "check-feedback.mjs failed (exit $LASTEXITCODE)" }
