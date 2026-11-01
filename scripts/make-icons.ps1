# Draws the app and tray icons into assets\. Run it after changing the logo;
# the results are checked in, so a normal build does not need it.
#
# Small sizes are not scaled down from the big logo: the bars would blur into
# grey. Each gets its own layout on whole pixels, so the gaps stay crisp.
param(
    [string]$Assets = "$PSScriptRoot\..\assets",
    [string]$Preview = ''
)

Add-Type -AssemblyName System.Drawing

$TileColor = [System.Drawing.ColorTranslator]::FromHtml('#0B0B0F')
$White = [System.Drawing.Color]::White
$Black = [System.Drawing.ColorTranslator]::FromHtml('#111111')

function Add-Pill($Path, [double]$X, [double]$Y, [double]$Width, [double]$Height) {
    $Diameter = [math]::Min($Width, $Height)
    $Figure = New-Object System.Drawing.Drawing2D.GraphicsPath
    if ($Width -ge $Height) {
        $Figure.AddArc([single]$X, [single]$Y, [single]$Diameter, [single]$Diameter, 90, 180)
        $Figure.AddArc([single]($X + $Width - $Diameter), [single]$Y, [single]$Diameter, [single]$Diameter, 270, 180)
    } else {
        $Figure.AddArc([single]$X, [single]$Y, [single]$Diameter, [single]$Diameter, 180, 180)
        $Figure.AddArc([single]$X, [single]($Y + $Height - $Diameter), [single]$Diameter, [single]$Diameter, 0, 180)
    }
    $Figure.CloseFigure()
    $Path.AddPath($Figure, $false)
}

# The pill with its bars cut out. Filling it with the alternate rule leaves real
# holes, so the bars show whatever is behind the icon.
function Get-Mark([int]$Size, [double]$X, [double]$Y) {
    $Path = New-Object System.Drawing.Drawing2D.GraphicsPath ([System.Drawing.Drawing2D.FillMode]::Alternate)
    switch ($Size) {
        16 {
            Add-Pill $Path ($X + 1) ($Y + 4) 14 8
            foreach ($Bar in @(@(4, 4), @(7, 6), @(10, 2))) {
                $Path.AddRectangle((New-Object System.Drawing.RectangleF ([single]($X + $Bar[0])), ([single]($Y + 8 - $Bar[1] / 2)), 2, ([single]$Bar[1])))
            }
        }
        20 {
            Add-Pill $Path ($X + 1) ($Y + 5) 18 10
            foreach ($Bar in @(@(5, 4), @(9, 8), @(13, 2))) {
                $Path.AddRectangle((New-Object System.Drawing.RectangleF ([single]($X + $Bar[0])), ([single]($Y + 10 - $Bar[1] / 2)), 2, ([single]$Bar[1])))
            }
        }
        24 {
            Add-Pill $Path ($X + 1) ($Y + 6) 22 12
            foreach ($Bar in @(@(5, 4), @(9, 8), @(13, 6), @(17, 2))) {
                $Path.AddRectangle((New-Object System.Drawing.RectangleF ([single]($X + $Bar[0])), ([single]($Y + 12 - $Bar[1] / 2)), 2, ([single]$Bar[1])))
            }
        }
        default {
            $Unit = $Size / 32.0
            Add-Pill $Path ($X + 2 * $Unit) ($Y + 8.5 * $Unit) (28 * $Unit) (15 * $Unit)
            foreach ($Bar in @(@(9, 5), @(13.2, 9), @(17.4, 6.6), @(21.6, 4))) {
                Add-Pill $Path ($X + $Bar[0] * $Unit) ($Y + (16 - $Bar[1] / 2) * $Unit) (2.4 * $Unit) ($Bar[1] * $Unit)
            }
        }
    }
    $Path
}

function New-Canvas([int]$Size) {
    $Bitmap = New-Object System.Drawing.Bitmap $Size, $Size, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $Graphics = [System.Drawing.Graphics]::FromImage($Bitmap)
    $Graphics.SmoothingMode = 'AntiAlias'
    $Graphics.PixelOffsetMode = 'HighQuality'
    $Graphics.Clear([System.Drawing.Color]::Transparent)
    @{ Bitmap = $Bitmap; Graphics = $Graphics }
}

function Get-Png($Canvas) {
    $Stream = New-Object IO.MemoryStream
    $Canvas.Bitmap.Save($Stream, [System.Drawing.Imaging.ImageFormat]::Png)
    $Canvas.Graphics.Dispose()
    $Canvas.Bitmap.Dispose()
    , $Stream.ToArray()
}

function New-RoundedSquare([int]$Size) {
    $Radius = $Size * 0.23
    $Diameter = [single]($Radius * 2)
    $Far = [single]($Size - $Diameter)
    $Path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $Path.AddArc(0, 0, $Diameter, $Diameter, 180, 90)
    $Path.AddArc($Far, 0, $Diameter, $Diameter, 270, 90)
    $Path.AddArc($Far, $Far, $Diameter, $Diameter, 0, 90)
    $Path.AddArc(0, $Far, $Diameter, $Diameter, 90, 90)
    $Path.CloseFigure()
    $Path
}

function New-AppIcon([int]$Size) {
    $Canvas = New-Canvas $Size
    $Canvas.Graphics.FillPath((New-Object System.Drawing.SolidBrush $TileColor), (New-RoundedSquare $Size))
    # Below 32 px there is no room for a margin around the mark.
    $Mark = if ($Size -le 24) { $Size } else { [int][math]::Round($Size * 0.76) }
    $Offset = ($Size - $Mark) / 2.0
    $Canvas.Graphics.FillPath((New-Object System.Drawing.SolidBrush $White), (Get-Mark $Mark $Offset $Offset))
    Get-Png $Canvas
}

function New-TrayIcon([int]$Size, $Color) {
    $Canvas = New-Canvas $Size
    $Canvas.Graphics.FillPath((New-Object System.Drawing.SolidBrush $Color), (Get-Mark $Size 0 0))
    Get-Png $Canvas
}

# An .ico is a small directory followed by the images, stored here as PNG.
function Write-Ico([string]$Path, $Images) {
    $Memory = New-Object IO.MemoryStream
    $Writer = New-Object IO.BinaryWriter $Memory
    $Writer.Write([uint16]0); $Writer.Write([uint16]1); $Writer.Write([uint16]$Images.Count)
    $Offset = 6 + 16 * $Images.Count
    foreach ($Image in $Images) {
        $Edge = if ($Image.Size -ge 256) { 0 } else { $Image.Size }
        $Writer.Write([byte]$Edge); $Writer.Write([byte]$Edge)
        $Writer.Write([byte]0); $Writer.Write([byte]0)
        $Writer.Write([uint16]1); $Writer.Write([uint16]32)
        $Writer.Write([uint32]$Image.Png.Length); $Writer.Write([uint32]$Offset)
        $Offset += $Image.Png.Length
    }
    foreach ($Image in $Images) { $Writer.Write([byte[]]$Image.Png) }
    [IO.File]::WriteAllBytes($Path, $Memory.ToArray())
}

New-Item -ItemType Directory -Force $Assets | Out-Null
$Assets = (Resolve-Path $Assets).Path

$AppImages = foreach ($Size in 16, 20, 24, 32, 40, 48, 64, 96, 256) {
    [pscustomobject]@{ Size = $Size; Png = (New-AppIcon $Size) }
}
Write-Ico (Join-Path $Assets 'app.ico') $AppImages

foreach ($Variant in @(@('tray-white.ico', $White), @('tray-black.ico', $Black))) {
    $TrayImages = foreach ($Size in 16, 20, 24, 32, 40, 48) {
        [pscustomobject]@{ Size = $Size; Png = (New-TrayIcon $Size $Variant[1]) }
    }
    Write-Ico (Join-Path $Assets $Variant[0]) $TrayImages
}

if ($Preview) {
    New-Item -ItemType Directory -Force $Preview | Out-Null
    $Sheet = New-Object System.Drawing.Bitmap 760, 360
    $Paint = [System.Drawing.Graphics]::FromImage($Sheet)
    $Paint.InterpolationMode = 'NearestNeighbor'
    $Paint.PixelOffsetMode = 'Half'
    $Paint.Clear([System.Drawing.Color]::FromArgb(255, 60, 60, 66))
    $Paint.FillRectangle((New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(28, 28, 28))), 0, 180, 380, 180)
    $Paint.FillRectangle((New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(243, 243, 243))), 380, 180, 380, 180)

    function Show-Image([byte[]]$Png, [int]$X, [int]$Y, [int]$Zoom) {
        $Image = [System.Drawing.Image]::FromStream((New-Object IO.MemoryStream (, $Png)))
        $Paint.DrawImage($Image, $X, $Y, $Image.Width * $Zoom, $Image.Height * $Zoom)
        $Image.Dispose()
    }

    $Paint.InterpolationMode = 'HighQualityBicubic'
    Show-Image ($AppImages | Where-Object Size -eq 256).Png 12 12 1
    $Paint.InterpolationMode = 'NearestNeighbor'
    $X = 290
    foreach ($Image in @($AppImages | Where-Object { $_.Size -in 16, 20, 24, 32 })) {
        Show-Image $Image.Png $X 20 4
        $X += $Image.Size * 4 + 14
    }
    $X = 16
    foreach ($Size in 16, 20, 24) {
        Show-Image (New-TrayIcon $Size $White) $X 196 5
        Show-Image (New-TrayIcon $Size $Black) ($X + 380) 196 5
        Show-Image (New-TrayIcon $Size $White) $X 320 1
        Show-Image (New-TrayIcon $Size $Black) ($X + 380) 320 1
        $X += $Size * 5 + 14
    }
    $Paint.Dispose()
    $Sheet.Save((Join-Path $Preview 'icons.png'), [System.Drawing.Imaging.ImageFormat]::Png)
    $Sheet.Dispose()
}

Get-ChildItem $Assets -Filter *.ico | ForEach-Object { '{0}: {1:N0} KB' -f $_.Name, ($_.Length / 1KB) }
