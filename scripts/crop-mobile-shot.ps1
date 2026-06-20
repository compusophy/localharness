# crop-mobile-shot.ps1 — turn a Claude-in-Chrome `gif_creator` export into a
# clean light-mode mobile README screenshot.
#
# The browser app renders the `?preview=mobile` column as a fixed 390px-wide
# `#root` centered in the (maximized, 1920px) desktop viewport. gif_creator
# downloads a GIF of the FULL viewport; this script lifts the final settled
# frame and crops out just that centered column, writing a PNG.
#
# Geometry: column width = 390 CSS px, viewport = 1920 CSS px, so in an image of
# width W the column is W*390/1920 wide, centered → left = W*(1920-390)/(2*1920).
# Frame-time waits during recording settle the wasm mount + light theme, so the
# LAST frame is the loaded view.
#
#   pwsh scripts/crop-mobile-shot.ps1 -Gif "$HOME/Downloads/lh-chat.gif" -Out web/screenshots/chat.png
param(
  [Parameter(Mandatory=$true)][string]$Gif,
  [Parameter(Mandatory=$true)][string]$Out,
  [int]$ViewportW = 1920,   # CSS-px width of the captured viewport (innerWidth)
  [int]$ColumnW   = 390,    # html.preview-mobile #root width (CSS px)
  [int]$RootLeft  = -1,     # measured #root left in CSS px (exact crop; -1 = assume centered)
  [int]$Margin    = 0,      # extra CSS px kept on each side (avoids modal edge clip)
  [int]$Frame     = -1      # which GIF frame; -1 = last (settled) frame
)
# Robust crop: pass the JS-measured innerWidth (-ViewportW) and #root left
# (-RootLeft) for an exact crop regardless of viewport/zoom. Without -RootLeft we
# fall back to assuming the 390px column is centered in -ViewportW.
Add-Type -AssemblyName System.Drawing
$img = [System.Drawing.Image]::FromFile((Resolve-Path $Gif))
try {
  $fd = New-Object System.Drawing.Imaging.FrameDimension $img.FrameDimensionsList[0]
  $count = $img.GetFrameCount($fd)
  $idx = if ($Frame -lt 0) { $count - 1 } else { [Math]::Min($Frame, $count - 1) }
  $img.SelectActiveFrame($fd, $idx) | Out-Null
  $scale = $img.Width / [double]$ViewportW
  $leftCss = if ($RootLeft -ge 0) { $RootLeft } else { ($ViewportW - $ColumnW) / 2.0 }
  $leftCss = [Math]::Max(0, $leftCss - $Margin)
  $cx = [int][Math]::Round($leftCss * $scale)
  $cw = [int][Math]::Min($img.Width - $cx, [int][Math]::Round(($ColumnW + 2 * $Margin) * $scale))
  $bmp = New-Object System.Drawing.Bitmap($img)
  try {
    $rect = New-Object System.Drawing.Rectangle($cx, 0, $cw, $img.Height)
    $crop = $bmp.Clone($rect, $bmp.PixelFormat)
    try {
      $dir = Split-Path -Parent $Out
      if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
      $crop.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
      Write-Output "wrote $Out  ($($crop.Width)x$($crop.Height)) from frame $idx/$count of $($img.Width)x$($img.Height)"
    } finally { $crop.Dispose() }
  } finally { $bmp.Dispose() }
} finally { $img.Dispose() }
