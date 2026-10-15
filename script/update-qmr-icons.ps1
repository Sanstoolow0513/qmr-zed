#Requires -Version 7.0
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$resources = Join-Path $PSScriptRoot '../crates/zed/resources'
$source = [Drawing.Bitmap]::new((Join-Path $resources 'app-icon-source.png'))

function ConvertTo-IconPng([int]$Size) {
    $bitmap = [Drawing.Bitmap]::new($Size, $Size, [Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    $stream = [IO.MemoryStream]::new()
    try {
        $graphics.CompositingMode = [Drawing.Drawing2D.CompositingMode]::SourceCopy
        $graphics.InterpolationMode = [Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $graphics.PixelOffsetMode = [Drawing.Drawing2D.PixelOffsetMode]::HighQuality
        $graphics.DrawImage($source, [Drawing.Rectangle]::new(0, 0, $Size, $Size))
        $bitmap.Save($stream, [Drawing.Imaging.ImageFormat]::Png)
        return ,$stream.ToArray()
    } finally {
        $stream.Dispose()
        $graphics.Dispose()
        $bitmap.Dispose()
    }
}

try {
    $standard = ConvertTo-IconPng 512
    $retina = ConvertTo-IconPng 1024
    $sizes = @(16, 20, 24, 32, 48, 64, 128, 256)
    $frames = @($sizes | ForEach-Object { ,(ConvertTo-IconPng $_) })
    $iconStream = [IO.MemoryStream]::new()
    $writer = [IO.BinaryWriter]::new($iconStream)
    try {
        $writer.Write([uint16]0)
        $writer.Write([uint16]1)
        $writer.Write([uint16]$frames.Count)
        $offset = 6 + 16 * $frames.Count
        for ($index = 0; $index -lt $frames.Count; $index++) {
            $dimension = if ($sizes[$index] -eq 256) { 0 } else { $sizes[$index] }
            $writer.Write([byte]$dimension)
            $writer.Write([byte]$dimension)
            $writer.Write([byte]0)
            $writer.Write([byte]0)
            $writer.Write([uint16]1)
            $writer.Write([uint16]32)
            $writer.Write([uint32]$frames[$index].Length)
            $writer.Write([uint32]$offset)
            $offset += $frames[$index].Length
        }
        foreach ($frame in $frames) { $writer.Write([byte[]]$frame) }
        $writer.Flush()
        $icon = $iconStream.ToArray()
        foreach ($channel in @('', '-dev', '-nightly', '-preview')) {
            [IO.File]::WriteAllBytes((Join-Path $resources "app-icon$channel.png"), $standard)
            [IO.File]::WriteAllBytes((Join-Path $resources "app-icon$channel@2x.png"), $retina)
            [IO.File]::WriteAllBytes((Join-Path $resources "windows/app-icon$channel.ico"), $icon)
        }
    } finally {
        $writer.Dispose()
        $iconStream.Dispose()
    }
} finally {
    $source.Dispose()
}
