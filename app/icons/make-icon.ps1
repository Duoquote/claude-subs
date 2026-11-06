# Draws app/icons/icon.ico: the Claude Subs mark (a slate rounded square with two meter bars,
# amber over green) as PNG frames 16, 32, 48 and 256 px. Run once after a design change:
#   powershell -NoProfile -ExecutionPolicy Bypass -File app/icons/make-icon.ps1
# The tray icon itself is drawn in code (app/src/icon.rs).

Add-Type -AssemblyName System.Drawing

function New-RoundRect([float]$x, [float]$y, [float]$w, [float]$h, [float]$r) {
    $d = 2 * $r
    $p = New-Object System.Drawing.Drawing2D.GraphicsPath
    $p.AddArc($x, $y, $d, $d, 180, 90)
    $p.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
    $p.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90)
    $p.AddArc($x, $y + $h - $d, $d, $d, 90, 90)
    $p.CloseFigure()
    return $p
}

$frames = foreach ($size in 16, 32, 48, 256) {
    $s = [float]$size
    $bmp = New-Object System.Drawing.Bitmap $size, $size, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.Clear([System.Drawing.Color]::Transparent)
    $slate = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(0x1F, 0x29, 0x37))
    $track = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(70, 255, 255, 255))
    $amber = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(0xF5, 0xB7, 0x32))
    $green = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(0x4A, 0xDE, 0x80))
    $g.FillPath($slate, (New-RoundRect 0 0 $s $s (0.22 * $s)))
    $x = 0.18 * $s; $w = 0.64 * $s; $h = 0.16 * $s
    $g.FillRectangle($track, $x, 0.30 * $s, $w, $h)
    $g.FillRectangle($amber, $x, 0.30 * $s, 0.70 * $w, $h)
    $g.FillRectangle($track, $x, 0.54 * $s, $w, $h)
    $g.FillRectangle($green, $x, 0.54 * $s, 0.40 * $w, $h)
    $g.Dispose()
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    [pscustomobject]@{ Size = $size; Png = $ms.ToArray() }
}

# ICO container: header, one 16-byte entry per frame, then the PNG payloads (Windows Vista+).
$out = New-Object System.IO.MemoryStream
$bw = New-Object System.IO.BinaryWriter $out
$bw.Write([uint16]0)
$bw.Write([uint16]1)
$bw.Write([uint16]$frames.Count)
$offset = 6 + 16 * $frames.Count
foreach ($f in $frames) {
    $dim = if ($f.Size -ge 256) { [byte]0 } else { [byte]$f.Size }
    $bw.Write($dim)
    $bw.Write($dim)
    $bw.Write([byte]0)
    $bw.Write([byte]0)
    $bw.Write([uint16]1)
    $bw.Write([uint16]32)
    $bw.Write([uint32]$f.Png.Length)
    $bw.Write([uint32]$offset)
    $offset += $f.Png.Length
}
foreach ($f in $frames) { $bw.Write($f.Png) }
$bw.Flush()

$target = Join-Path $PSScriptRoot 'icon.ico'
[System.IO.File]::WriteAllBytes($target, $out.ToArray())
Write-Output "wrote $target"
