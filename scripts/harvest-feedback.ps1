# Read every feedback submission from CONTRACT STATE on the registry
# diamond. One row per submission: <index>  <unix-ts>  <sender>  <text>.
# Reads via view functions (no event-log scraping, so the Tempo 100k-block
# log window no longer hides older notes).
#
# Usage:
#   pwsh scripts/harvest-feedback.ps1
#
# Env overrides: DIAMOND, RPC.

$ErrorActionPreference = "Stop"

if (-not $env:DIAMOND) { $env:DIAMOND = "0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c" }
if (-not $env:RPC)     { $env:RPC     = "https://rpc.moderato.tempo.xyz" }

$countRaw = (cast call $env:DIAMOND "feedbackCount()(uint256)" --rpc-url $env:RPC)
# cast may append units in parentheses (e.g. "3 [3e0]"); keep the integer.
$count = [int64](($countRaw -split '\s+')[0])

if ($count -eq 0) {
    Write-Output "no feedback yet"
    return
}

for ($i = 0; $i -lt $count; $i++) {
    # feedbackAt returns (address sender, uint64 timestamp, string text),
    # one value per line.
    $out = @(cast call $env:DIAMOND "feedbackAt(uint256)(address,uint64,string)" $i --rpc-url $env:RPC)
    $sender = $out[0]
    $ts = (($out[1] -split '\s+')[0])
    $text = ($out[2..($out.Length - 1)] -join "`n")
    Write-Output ("{0}`t{1}`t{2}`t{3}" -f $i, $ts, $sender, $text)
}
