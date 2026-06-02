# Pull every `FeedbackSubmitted` event from the registry diamond.
# One row per submission: <iso-ts>  <sender>  <text>
#
# Usage:
#   pwsh scripts/harvest-feedback.ps1
#
# Env overrides: DIAMOND, RPC.

$ErrorActionPreference = "Stop"

if (-not $env:DIAMOND) { $env:DIAMOND = "0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c" }
if (-not $env:RPC)     { $env:RPC     = "https://rpc.moderato.tempo.xyz" }

# Tempo caps eth_getLogs to a 100k-block window. Default to scanning
# the most recent ~99k blocks; set $env:FROM_BLOCK to widen.
$latest = [int64](cast block-number --rpc-url $env:RPC)
if (-not $env:FROM_BLOCK) {
    $from = [Math]::Max(0, $latest - 99000)
} else {
    $from = [int64]$env:FROM_BLOCK
}

cast logs `
    --address $env:DIAMOND `
    --rpc-url $env:RPC `
    --from-block $from `
    --to-block $latest `
    'event FeedbackSubmitted(address indexed sender, uint256 timestamp, string text)'
