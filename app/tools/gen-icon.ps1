# 生成安卓启动图标：警示橙红渐变圆角方块 + 白色铃铛剪影
Add-Type -AssemblyName System.Drawing

function Add-Poly([System.Drawing.Drawing2D.GraphicsPath]$path, [double[]]$flat, [float]$s) {
    $n = [int]($flat.Count / 2)
    $pts = New-Object 'System.Drawing.PointF[]' $n
    for ($i = 0; $i -lt $n; $i++) {
        $pts[$i] = [System.Drawing.PointF]::new([float]($flat[2 * $i] * $s), [float]($flat[2 * $i + 1] * $s))
    }
    $path.AddLines($pts)
    $path.CloseFigure()
}

function New-IconPng([int]$size, [string]$outPath) {
    $bmp = New-Object System.Drawing.Bitmap($size, $size)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality

    # 圆角矩形裁剪（radius 18%）
    $r = [int]($size * 0.18)
    $d = $r * 2
    $gp = New-Object System.Drawing.Drawing2D.GraphicsPath
    $gp.AddArc(0, 0, $d, $d, 180, 90)
    $gp.AddArc($size - $d, 0, $d, $d, 270, 90)
    $gp.AddArc($size - $d, $size - $d, $d, $d, 0, 90)
    $gp.AddArc(0, $size - $d, $d, $d, 90, 90)
    $gp.CloseFigure()
    $g.SetClip($gp)

    # 警示渐变背景（orange-500 → red-700，135°）
    $rect = New-Object System.Drawing.Rectangle(0, 0, $size, $size)
    $brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush(
        $rect,
        [System.Drawing.Color]::FromArgb(255, 0xF9, 0x73, 0x16),
        [System.Drawing.Color]::FromArgb(255, 0xDC, 0x26, 0x26),
        45.0)
    $g.FillRectangle($brush, $rect)

    # 柔和装饰圆斑
    $deco1 = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(26, 255, 255, 255))
    $g.FillEllipse($deco1, [float]($size * 0.58), [float](-$size * 0.28), [float]($size * 0.60), [float]($size * 0.60))
    $deco2 = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(18, 255, 255, 255))
    $g.FillEllipse($deco2, [float]($size * 0.40), [float]($size * 0.60), [float]($size * 0.46), [float]($size * 0.46))

    # 白色铃铛剪影（24 网格多边形近似）+ 下方铃锤
    $s = $size / 24.0
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $path.FillMode = [System.Drawing.Drawing2D.FillMode]::Alternate
    Add-Poly $path ([double[]]@(
        12, 1.6, 13.4, 1.9, 14.8, 2.9, 16.0, 4.6, 16.9, 6.8, 17.5, 9.2, 17.8, 11.6,
        18.0, 13.8, 19.0, 15.6, 20.1, 16.8, 20.1, 18.6, 3.9, 18.6, 3.9, 16.8,
        5.0, 15.6, 6.0, 13.8, 6.2, 11.6, 6.5, 9.2, 7.1, 6.8, 8.0, 4.6, 9.2, 2.9, 10.6, 1.9
    )) $s
    Add-Poly $path ([double[]]@(9.7, 19.9, 14.3, 19.9, 12.0, 22.6)) $s

    $g.FillPath([System.Drawing.Brushes]::White, $path)

    $g.Dispose()

    $dir = Split-Path $outPath
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    $bmp.Save($outPath, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    Write-Output "icon: $outPath ($size px)"
}

$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)

New-IconPng 192 "$root\res\mipmap-xxxhdpi\ic_launcher.png"

foreach ($m in @(
        @{ d = "xxhdpi"; s = 144 },
        @{ d = "xhdpi"; s = 96 },
        @{ d = "hdpi"; s = 72 },
        @{ d = "mdpi"; s = 48 }
    )) {
    $src = [System.Drawing.Image]::FromFile("$root\res\mipmap-xxxhdpi\ic_launcher.png")
    $bmp = New-Object System.Drawing.Bitmap($m.s, $m.s)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.DrawImage($src, 0, 0, $m.s, $m.s)
    $g.Dispose()
    $dir = "$root\res\mipmap-$($m.d)"
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    $bmp.Save("$dir\ic_launcher.png", [System.Drawing.Imaging.ImageFormat]::Png)
    $src.Dispose()
    $bmp.Dispose()
    Write-Output "icon: $dir\ic_launcher.png ($($m.s) px)"
}
Write-Output "done"
