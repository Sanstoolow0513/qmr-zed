#!/usr/bin/env pwsh

$ErrorActionPreference = 'Stop'

function Assert-Equal($Actual, $Expected, [string]$Message) {
    if ($Actual -cne $Expected) {
        throw "${Message}: expected '$Expected', got '$Actual'"
    }
}

function Assert-File([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "Missing staged file: $Path"
    }
}

$repository = Split-Path -Parent $PSScriptRoot
$taskRoot = [IO.Path]::GetFullPath((Join-Path $repository 'target'))
$testRoot = Join-Path $taskRoot "windows-bundle-test-$([guid]::NewGuid().ToString('N'))"
$savedEnvironment = @{}
foreach ($name in @('ZED_WORKSPACE', 'RELEASE_VERSION', 'GITHUB_ENV')) {
    $savedEnvironment[$name] = [Environment]::GetEnvironmentVariable($name)
}

# Import only the production functions; the script entry point configures build
# tools and downloads dependencies, which these fixture tests must not execute.
$parseErrors = $null
$tokens = $null
$bundle = [Management.Automation.Language.Parser]::ParseFile(
    (Join-Path $PSScriptRoot 'bundle-windows.ps1'), [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count) { throw $parseErrors[0] }
foreach ($name in @('BuildZedAndItsFriends', 'MakeAppx', 'CollectFiles', 'BuildInstaller')) {
    $definition = $bundle.Find({
        param($node)
        $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name
    }, $false)
    if (-not $definition) { throw "Missing bundler function: $name" }
    . ([scriptblock]::Create($definition.Extent.Text))
}

function cargo {
    $script:cargoCalls += ,@($args)
}

function Get-ChildItem {
    param([string]$Path, [switch]$Recurse, [string]$Filter)
    Assert-Equal $Path 'C:\Program Files (x86)\Windows Kits\10\bin' 'SDK discovery path'
    Assert-Equal $Filter 'makeappx.exe' 'SDK discovery filter'
    [pscustomobject]@{ FullName = $script:fakeMakeAppx }
}

function Start-Process {
    param([string]$FilePath, [string[]]$ArgumentList, [switch]$NoNewWindow, [switch]$Wait, [switch]$PassThru)
    Assert-Equal (Split-Path -Leaf $FilePath) 'ISCC.exe' 'Only Inno Setup may be invoked'
    Assert-File $ArgumentList[0]
    $script:compilerDefinitions = @{}
    foreach ($argument in $ArgumentList | Select-Object -Skip 1) {
        if ($argument -notmatch '^/d([^=]+)="(.*)"$') { throw "Unexpected compiler argument: $argument" }
        $script:compilerDefinitions[$Matches[1]] = $Matches[2]
    }
    [pscustomobject]@{ ExitCode = 0 }
}

try {
    New-Item -ItemType Directory -Path $testRoot -Force | Out-Null
    $fakeMakeAppx = Join-Path $testRoot 'sdk\x64\makeappx.exe'
    Set-Item -LiteralPath "Function:$fakeMakeAppx" -Value {
        Assert-Equal $args[0] 'pack' 'Appx command'
        $manifestDirectory = $args[[Array]::IndexOf($args, '/d') + 1]
        Assert-File (Join-Path $manifestDirectory 'AppxManifest.xml')
        Set-Content -LiteralPath $args[[Array]::IndexOf($args, '/p') + 1] -Value 'appx fixture'
    }

    foreach ($Architecture in @('x86_64', 'aarch64')) {
        foreach ($channel in @('stable', 'preview', 'nightly', 'dev')) {
            $caseRoot = Join-Path $testRoot "$Architecture-$channel"
            $env:ZED_WORKSPACE = $caseRoot
            $env:RELEASE_VERSION = '1.2.3'
            $env:GITHUB_ENV = $null
            $CargoOutDir = "target/$Architecture-pc-windows-msvc/release"
            $target = "$Architecture-pc-windows-msvc"
            $innoDir = Join-Path $caseRoot 'inno'
            foreach ($directory in @($CargoOutDir, 'inno', 'inno/make_appx', 'inno/appx', 'inno/bin', 'inno/tools',
                    'crates/explorer_command_injector', 'AGS_SDK-6.3.0/ags_lib/lib',
                    'conpty/build/native/runtimes/x64', 'conpty/build/native/runtimes/arm64',
                    'conpty/runtimes/win-x64/native', 'conpty/runtimes/win-arm64/native')) {
                New-Item -ItemType Directory -Path (Join-Path $caseRoot $directory) -Force | Out-Null
            }
            Copy-Item -Path "$repository/crates/explorer_command_injector/AppxManifest*.xml" -Destination "$caseRoot/crates/explorer_command_injector"
            foreach ($file in @('zed.sh', 'zed.iss')) {
                Copy-Item -LiteralPath "$repository/crates/zed/resources/windows/$file" -Destination $innoDir
            }
            foreach ($file in @('qmrzed.exe', 'zed.exe', 'cli.exe', 'auto_update_helper.exe', 'explorer_command_injector.dll')) {
                Set-Content -LiteralPath (Join-Path "$caseRoot/$CargoOutDir" $file) -Value $file -NoNewline
            }
            foreach ($file in @('AGS_SDK-6.3.0/ags_lib/lib/amd_ags_x64.dll',
                    'conpty/build/native/runtimes/x64/OpenConsole.exe', 'conpty/build/native/runtimes/arm64/OpenConsole.exe',
                    'conpty/runtimes/win-x64/native/conpty.dll', 'conpty/runtimes/win-arm64/native/conpty.dll')) {
                Set-Content -LiteralPath (Join-Path $caseRoot $file) -Value 'dependency fixture'
            }

            $script:cargoCalls = @()
            $script:compilerDefinitions = @{}
            $script:buildSuccess = $false
            $script:setupPath = $null
            $canCodeSign = $false
            Push-Location $caseRoot
            try {
                BuildZedAndItsFriends | Out-Null
                MakeAppx | Out-Null
                CollectFiles | Out-Null
                BuildInstaller 6>$null | Out-Null
            } finally {
                Pop-Location
            }

            foreach ($file in @('QMRZed.exe', 'bin/qmrzed.exe', 'bin/qmrzed', 'tools/auto_update_helper.exe',
                    'appx/zed_explorer_command_injector.appx', 'appx/zed_explorer_command_injector.dll')) {
                Assert-File (Join-Path $innoDir $file)
            }
            Assert-Equal (Get-Content "$innoDir/QMRZed.exe" -Raw) 'qmrzed.exe' 'Prefer the fork executable'
            Assert-Equal (Get-Content "$innoDir/bin/qmrzed.exe" -Raw) 'cli.exe' 'Install the CLI launcher'
            Assert-Equal (Get-Content "$innoDir/bin/qmrzed" -Raw) (Get-Content "$repository/crates/zed/resources/windows/zed.sh" -Raw) 'Install the shell launcher'

            [xml]$manifest = Get-Content "$innoDir/make_appx/AppxManifest.xml" -Raw
            $application = $manifest.SelectSingleNode('//*[local-name()="Application"]')
            $comClass = $manifest.SelectSingleNode('//*[local-name()="Class"]')
            Assert-File (Join-Path $innoDir $application.Executable)
            Assert-File (Join-Path $innoDir $comClass.Path)
            foreach ($verb in $manifest.SelectNodes('//*[local-name()="Verb"]')) {
                Assert-Equal $verb.Clsid $comClass.Id 'Context-menu COM registration'
            }
            $identity = 'Sanstoolow0513.QMRZed'
            if ($channel -ne 'stable') { $identity += '.' + (Get-Culture).TextInfo.ToTitleCase($channel) }
            Assert-Equal $manifest.Package.Identity.Name $identity 'Channel package identity'
            Assert-Equal $compilerDefinitions['AppxName'] $identity 'Installer package identity'
            Assert-Equal $compilerDefinitions['AppExeName'] ([IO.Path]::GetFileNameWithoutExtension($application.Executable)) 'Installer executable matches manifest'
            Assert-Equal $compilerDefinitions['AppSetupName'] "QMRZed-$Architecture" 'Branded installer filename'
            Assert-Equal $setupPath (Join-Path $caseRoot "target/QMRZed-$Architecture.exe") 'Local installer path without GITHUB_ENV'
            Assert-Equal $buildSuccess $true 'Installer success'
            Assert-Equal $cargoCalls.Count 2 'Build editor and Explorer integration'
            foreach ($call in $cargoCalls) {
                Assert-Equal $call[[Array]::IndexOf($call, '--target') + 1] $target 'Cargo target architecture'
            }
            $explorerBuild = $cargoCalls[1]
            $featureIndex = [Array]::IndexOf($explorerBuild, '--features')
            $expectedFeature = if ($channel -in @('stable', 'preview')) { $channel } else { '' }
            $selectedFeature = if ($featureIndex -ge 0) { $explorerBuild[$featureIndex + 1] } else { '' }
            Assert-Equal $selectedFeature $expectedFeature 'Explorer DLL channel feature'
            Assert-Equal ($explorerBuild -contains '--no-default-features') ($channel -ne 'nightly') 'Isolate Explorer DLL channel'
            Write-Host "PASS: $Architecture / $channel"
        }
    }
} finally {
    foreach ($name in $savedEnvironment.Keys) {
        [Environment]::SetEnvironmentVariable($name, $savedEnvironment[$name])
    }
    $resolvedTestRoot = [IO.Path]::GetFullPath($testRoot)
    if (-not $resolvedTestRoot.StartsWith($taskRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to remove test directory outside $taskRoot"
    }
    if (Test-Path -LiteralPath $resolvedTestRoot) {
        Remove-Item -LiteralPath $resolvedTestRoot -Recurse -Force
    }
}
