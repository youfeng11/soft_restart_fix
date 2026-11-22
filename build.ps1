#Requires -Version 5.1
[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [string]$NdkPath,

    [ValidateRange(21, 2147483647)]
    [int]$ApiLevel = $(if ($env:API_LEVEL) { $env:API_LEVEL } else { 21 })
)

$ErrorActionPreference = 'Stop'

function Test-Ndk([string]$Path) {
    if (-not $Path) { return $false }
    return Test-Path -LiteralPath (Join-Path $Path 'toolchains/llvm/prebuilt/windows-x86_64/bin/clang.exe') -PathType Leaf
}

function Find-Ndk([string]$RequestedPath) {
    if ($RequestedPath) {
        if (-not (Test-Ndk $RequestedPath)) {
            throw "Invalid Windows NDK path: $RequestedPath"
        }
        return (Resolve-Path -LiteralPath $RequestedPath).ProviderPath
    }

    foreach ($candidate in @($env:ANDROID_NDK_HOME, $env:NDK_HOME, $env:ANDROID_NDK_ROOT, $env:ANDROID_NDK, $env:NDK_PATH)) {
        if (Test-Ndk $candidate) {
            return (Resolve-Path -LiteralPath $candidate).ProviderPath
        }
    }

    $sdkRoots = @($env:ANDROID_HOME, $env:ANDROID_SDK_ROOT)
    if ($env:LOCALAPPDATA) {
        $sdkRoots += Join-Path $env:LOCALAPPDATA 'Android/Sdk'
    }
    $adb = Get-Command adb -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($adb) {
        $sdkRoots += Split-Path (Split-Path $adb.Source -Parent) -Parent
    }

    $candidates = @(
        foreach ($sdkRoot in ($sdkRoots | Where-Object { $_ } | Select-Object -Unique)) {
            $ndkRoot = Join-Path $sdkRoot 'ndk'
            if (Test-Path -LiteralPath $ndkRoot -PathType Container) {
                Get-ChildItem -LiteralPath $ndkRoot -Directory | Where-Object { Test-Ndk $_.FullName }
            }
            if (Test-Ndk (Join-Path $sdkRoot 'ndk-bundle')) {
                Get-Item -LiteralPath (Join-Path $sdkRoot 'ndk-bundle')
            }
        }
    )
    $latest = $candidates | Sort-Object {
        try { [version]$_.Name } catch { [version]'0.0' }
    } -Descending | Select-Object -First 1
    if ($latest) { return $latest.FullName }

    throw 'Android NDK not found. Use .\build.ps1 -NdkPath "D:\SDK\ndk\26.1.10909125" or set ANDROID_NDK_HOME.'
}

function Invoke-Checked([string]$Executable, [string[]]$Arguments) {
    & $Executable @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$Executable exited with code $LASTEXITCODE."
    }
}

$cargo = Get-Command cargo -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
$rustup = Get-Command rustup -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $cargo -or -not $rustup) {
    throw 'Install Rust with rustup (https://rustup.rs), then reopen PowerShell so cargo and rustup are on PATH.'
}
$NdkPath = Find-Ndk $NdkPath
$toolchain = Join-Path $NdkPath 'toolchains/llvm/prebuilt/windows-x86_64/bin'
$linker = Join-Path $toolchain 'clang.exe'
$strip = Join-Path $toolchain 'llvm-strip.exe'
if (-not (Test-Path -LiteralPath $strip -PathType Leaf)) {
    throw "NDK tool not found: $strip"
}

$targets = @(
    @{ Rust = 'aarch64-linux-android'; Clang = 'aarch64-linux-android'; Output = 'pid_wrap_arm64' },
    @{ Rust = 'armv7-linux-androideabi'; Clang = 'armv7a-linux-androideabi'; Output = 'pid_wrap_armeabi' },
    @{ Rust = 'x86_64-linux-android'; Clang = 'x86_64-linux-android'; Output = 'pid_wrap_x86_64' },
    @{ Rust = 'i686-linux-android'; Clang = 'i686-linux-android'; Output = 'pid_wrap_x86' }
)
Push-Location -LiteralPath $PSScriptRoot
try {
    Invoke-Checked $cargo.Source @('--version')
    Write-Host "Using NDK: $NdkPath (API $ApiLevel)"
    $installedTargets = @(& $rustup.Source target list --installed)
    if ($LASTEXITCODE -ne 0) { throw 'Could not list installed Rust targets.' }

    $binDir = Join-Path $PSScriptRoot 'bin'
    $targetDir = Join-Path $PSScriptRoot 'target'
    New-Item -ItemType Directory -Path $binDir -Force | Out-Null
    foreach ($target in $targets) {
        if ($installedTargets -notcontains $target.Rust) {
            Invoke-Checked $rustup.Source @('target', 'add', $target.Rust)
        }

        Write-Host "Building $($target.Rust)..."
        $linkerVariable = 'CARGO_TARGET_' + $target.Rust.ToUpperInvariant().Replace('-', '_') + '_LINKER'
        $previousLinker = [Environment]::GetEnvironmentVariable($linkerVariable, 'Process')
        try {
            [Environment]::SetEnvironmentVariable($linkerVariable, $linker, 'Process')
            # Invoke clang.exe directly: NDK .cmd wrappers can break on spaced paths.
            Invoke-Checked $cargo.Source @('rustc', '--bin', 'pid_wrap', '--release', '--locked', '--target', $target.Rust, '--target-dir', $targetDir, '--', '-C', "link-arg=--target=$($target.Clang)$ApiLevel")
        } finally {
            [Environment]::SetEnvironmentVariable($linkerVariable, $previousLinker, 'Process')
        }

        $outputPath = Join-Path $binDir $target.Output
        Copy-Item -LiteralPath (Join-Path $targetDir "$($target.Rust)/release/pid_wrap") -Destination $outputPath -Force
        Invoke-Checked $strip @('-s', $outputPath)
    }

    # Use explicit entries and Unix paths; normalize Windows checkout line endings.
    Add-Type -AssemblyName System.IO.Compression
    $zipPath = Join-Path $PSScriptRoot 'soft_restart_fix.zip'
    $zipStream = [IO.File]::Open($zipPath, [IO.FileMode]::Create)
    try {
        $archive = [IO.Compression.ZipArchive]::new($zipStream, [IO.Compression.ZipArchiveMode]::Create, $true)
        try {
            $files = @('module.prop', 'customize.sh', 'emulated-soft-reboot.sh')
            $files += $targets | ForEach-Object { 'bin/' + $_.Output }
            foreach ($file in $files) {
                $entry = $archive.CreateEntry($file, [IO.Compression.CompressionLevel]::Optimal)
                $entry.ExternalAttributes = if ($file -eq 'module.prop') { 0x81A4 -shl 16 } else { 0x81ED -shl 16 }
                $entryStream = $entry.Open()
                try {
                    $sourcePath = Join-Path $PSScriptRoot $file
                    if ($file.StartsWith('bin/')) {
                        $source = [IO.File]::OpenRead($sourcePath)
                        try { $source.CopyTo($entryStream) } finally { $source.Dispose() }
                    } else {
                        $content = [IO.File]::ReadAllText($sourcePath).Replace("`r`n", "`n")
                        $bytes = [Text.UTF8Encoding]::new($false).GetBytes($content)
                        $entryStream.Write($bytes, 0, $bytes.Length)
                    }
                } finally { $entryStream.Dispose() }
            }
        } finally { $archive.Dispose() }
    } finally { $zipStream.Dispose() }
    Write-Host "Package created: $zipPath"
} finally {
    Pop-Location
}
