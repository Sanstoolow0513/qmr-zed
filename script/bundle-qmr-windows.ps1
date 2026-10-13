#Requires -Version 7.0
[CmdletBinding()]
param(
    [ValidateRange(1, 128)][int]$Jobs = 8,
    # Only for repackaging an already verified build of these sources.
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$workspace = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Push-Location $workspace
$savedEnvironment = @{}
$environmentNames = @('PATH', 'ZED_RELEASE_CHANNEL', 'RELEASE_CHANNEL', 'RELEASE_VERSION', 'ZED_UPDATE_EXPLANATION', 'CARGO_PROFILE_RELEASE_DEBUG')
foreach ($name in $environmentNames) {
    $savedEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}

try {
    if (-not $IsWindows -or [Runtime.InteropServices.RuntimeInformation]::OSArchitecture -ne 'X64') {
        throw 'This bundler currently supports Windows x64 only.'
    }
    $hostTriple = (rustc -vV | Select-String '^host: ').ToString().Substring(6)
    if ($hostTriple -ne 'x86_64-pc-windows-msvc') {
        throw "Expected the x86_64-pc-windows-msvc Rust toolchain, found $hostTriple."
    }
    if (-not (Get-Command cmake -ErrorAction SilentlyContinue)) {
        $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
        $visualStudio = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
        if (-not $visualStudio) { throw 'Visual Studio C++ build tools are required.' }
        $cmakeDirectory = Join-Path $visualStudio 'Common7/IDE/CommonExtensions/Microsoft/CMake/CMake/bin'
        $env:PATH = "$cmakeDirectory;$env:PATH"
        Get-Command cmake -ErrorAction Stop | Out-Null
    }

    $metadata = cargo metadata --no-deps --format-version 1 --locked | ConvertFrom-Json
    $version = ($metadata.packages | Where-Object name -EQ 'zed').version
    $env:ZED_RELEASE_CHANNEL = 'stable'
    $env:RELEASE_CHANNEL = 'stable'
    $env:RELEASE_VERSION = $version
    $env:ZED_UPDATE_EXPLANATION = 'QMR Zed is an independent fork. Download updates from https://github.com/Sanstoolow0513/qmr-zed/releases.'
    $env:CARGO_PROFILE_RELEASE_DEBUG = '0'

    if (-not $SkipBuild) {
        # Generate before compiling so the same attribution is embedded in the app.
        & "$PSScriptRoot/generate-licenses.ps1"
        cargo build --locked --release --package zed --package cli --jobs $Jobs
    }

    $releaseDirectory = Join-Path $metadata.target_directory 'release'
    $binaryVersion = (Get-Item -LiteralPath "$releaseDirectory/qmrzed.exe").VersionInfo.ProductVersion
    if (-not $binaryVersion.StartsWith("$version+stable.")) {
        throw "Expected a stable-channel QMR Zed $version build, found $binaryVersion."
    }
    $distributionDirectory = Join-Path $metadata.target_directory 'qmr-dist'
    New-Item -ItemType Directory -Path $distributionDirectory -Force | Out-Null
    $packageName = "qmr-zed-$version-windows-x86_64"
    # A fresh directory prevents stale runtime files from entering a new bundle.
    $stagingDirectory = Join-Path $distributionDirectory "$packageName-$([Guid]::NewGuid().ToString('N'))"
    $packageDirectory = Join-Path $stagingDirectory $packageName
    New-Item -ItemType Directory -Path "$packageDirectory/bin", "$packageDirectory/x64" -Force | Out-Null
    Copy-Item -LiteralPath "$releaseDirectory/qmrzed.exe" -Destination "$packageDirectory/QMRZed.exe"
    Copy-Item -LiteralPath "$releaseDirectory/cli.exe" -Destination "$packageDirectory/bin/qmrzed.exe"
    # The application locates this internal CLI for terminal/editor integration.
    Copy-Item -LiteralPath "$releaseDirectory/cli.exe" -Destination "$packageDirectory/cli.exe"
    Copy-Item -LiteralPath "$releaseDirectory/conpty.dll" -Destination $packageDirectory
    Copy-Item -LiteralPath "$releaseDirectory/OpenConsole.exe" -Destination "$packageDirectory/x64/OpenConsole.exe"
    Copy-Item -LiteralPath 'LICENSE-GPL', 'LICENSE-APACHE', 'assets/licenses.md' -Destination $packageDirectory
    foreach ($license in (Get-ChildItem assets -Recurse -File | Where-Object { $_.Name -match '^(LICENSE|OFL|COPYING)' })) {
        $relativePath = [IO.Path]::GetRelativePath((Join-Path $workspace 'assets'), $license.FullName)
        $licenseDestination = Join-Path "$packageDirectory/asset-licenses" $relativePath
        New-Item -ItemType Directory -Path (Split-Path $licenseDestination) -Force | Out-Null
        Copy-Item -LiteralPath $license.FullName -Destination $licenseDestination
    }
    Invoke-WebRequest 'https://raw.githubusercontent.com/microsoft/terminal/v1.25.1912.0/LICENSE' -OutFile "$packageDirectory/LICENSE-ConPTY.txt"

    # AGS is loaded dynamically on AMD GPUs; other GPUs don't use it.
    $agsArchive = Join-Path $stagingDirectory 'ags.zip'
    Invoke-WebRequest 'https://codeload.github.com/GPUOpen-LibrariesAndSDKs/AGS_SDK/zip/refs/tags/v6.3.0' -OutFile $agsArchive
    Expand-Archive -LiteralPath $agsArchive -DestinationPath "$stagingDirectory/ags"
    Copy-Item -LiteralPath "$stagingDirectory/ags/AGS_SDK-6.3.0/ags_lib/lib/amd_ags_x64.dll" -Destination $packageDirectory
    Copy-Item -LiteralPath "$stagingDirectory/ags/AGS_SDK-6.3.0/LICENSE.txt" -Destination "$packageDirectory/LICENSE-AMD-AGS.txt"

    $sourceName = "qmr-zed-$version-source.zip"
    $sourcePath = Join-Path $distributionDirectory $sourceName
    $sourceTemporaryPath = Join-Path $stagingDirectory $sourceName
    $sourceArchive = [IO.Compression.ZipFile]::Open($sourceTemporaryPath, [IO.Compression.ZipArchiveMode]::Create)
    try {
        $sourceFiles = @(git -c core.quotepath=false ls-files --cached --others --exclude-standard) + @('assets/licenses.md')
        foreach ($relativePath in ($sourceFiles | Sort-Object -Unique)) {
            if (Test-Path -LiteralPath $relativePath -PathType Leaf) {
                [IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
                    $sourceArchive, (Join-Path $workspace $relativePath), $relativePath,
                    [IO.Compression.CompressionLevel]::Optimal
                ) | Out-Null
            }
        }
    } finally {
        $sourceArchive.Dispose()
    }
    Copy-Item -LiteralPath $sourceTemporaryPath -Destination $sourcePath -Force

    $manifest = [ordered]@{
        name = 'QMR Zed'
        version = $version
        platform = $hostTriple
        repository = 'https://github.com/Sanstoolow0513/qmr-zed'
        branch = (git branch --show-current)
        base_commit = (git rev-parse HEAD)
        working_tree_changes = @(git status --porcelain)
        built_at_utc = [DateTime]::UtcNow.ToString('o')
        toolchain = (rustc --version)
        profile = 'release (optimized, thin LTO, debug symbols omitted)'
        source_archive = $sourceName
        source_sha256 = (Get-FileHash -LiteralPath $sourcePath -Algorithm SHA256).Hash.ToLowerInvariant()
        automatic_updates = $false
        signed = $false
    }
    $manifest | ConvertTo-Json -Depth 4 | Set-Content "$packageDirectory/build-info.json" -Encoding utf8NoBOM
    @"
QMR Zed $version - Windows x64

Independent development-focused fork of Zed, without built-in AI.
Extract the entire folder, then run QMRZed.exe.
For the command line, use bin\qmrzed.exe or add this folder's bin directory to PATH.

Settings: %APPDATA%\QMRZed
Data and logs: %LOCALAPPDATA%\QMRZed
Official Zed settings and data are separate. Copy settings manually if desired.
Updates: https://github.com/Sanstoolow0513/qmr-zed/releases
Updates are manual; this application cannot update to official Zed builds.

This archive is unsigned. It does not register file associations or replace Zed.
The source for this exact build is in the accompanying $sourceName archive.
See build-info.json, LICENSE-GPL, LICENSE-APACHE, licenses.md, asset-licenses,
LICENSE-ConPTY.txt and LICENSE-AMD-AGS.txt.
"@ | Set-Content "$packageDirectory/README.txt" -Encoding utf8NoBOM

    & "$packageDirectory/bin/qmrzed.exe" --version
    $archivePath = Join-Path $distributionDirectory "$packageName.zip"
    $archiveTemporaryPath = Join-Path $stagingDirectory "$packageName.zip"
    [IO.Compression.ZipFile]::CreateFromDirectory($packageDirectory, $archiveTemporaryPath, [IO.Compression.CompressionLevel]::Optimal, $true)
    Copy-Item -LiteralPath $archiveTemporaryPath -Destination $archivePath -Force
    @($archivePath, $sourcePath) | ForEach-Object {
        $hash = (Get-FileHash -LiteralPath $_ -Algorithm SHA256).Hash.ToLowerInvariant()
        "$hash  $([IO.Path]::GetFileName($_))"
    } | Set-Content "$distributionDirectory/SHA256SUMS.txt" -Encoding ascii
    Write-Output "Runnable folder: $packageDirectory"
    Write-Output "Release archive: $archivePath"
    Write-Output "Source archive: $sourcePath"
} finally {
    foreach ($name in $environmentNames) {
        [Environment]::SetEnvironmentVariable($name, $savedEnvironment[$name], 'Process')
    }
    Pop-Location
}
