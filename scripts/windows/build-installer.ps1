param(
    [switch]$SkipBuild,
    [string]$LibwdiDir = (Join-Path $env:LOCALAPPDATA 'DirectHCI\native'),
    [string]$OutputDir = (Join-Path $env:LOCALAPPDATA 'DirectHCI\installer-output')
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
if (-not $env:CARGO_TARGET_DIR) {
    $env:CARGO_TARGET_DIR = Join-Path $env:LOCALAPPDATA 'DirectHCI\target'
}
$binaryDir = Join-Path $env:CARGO_TARGET_DIR 'x86_64-pc-windows-gnu\release'
$libwdiDir = [System.IO.Path]::GetFullPath($LibwdiDir)
foreach ($file in @('libwdi.dll', 'libwdi-1.5.1-source.tar.gz', 'libwdi-build.json')) {
    $path = Join-Path $libwdiDir $file
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Missing libwdi provisioning component: $path. Run build-libwdi.ps1 on the build host first."
    }
}
$manifest = Get-Content -LiteralPath (Join-Path $libwdiDir 'libwdi-build.json') -Raw | ConvertFrom-Json
$scriptHash = (Get-FileHash -LiteralPath (Join-Path $PSScriptRoot 'build-libwdi.ps1') -Algorithm SHA256).Hash.ToLowerInvariant()
$dllHash = (Get-FileHash -LiteralPath (Join-Path $libwdiDir 'libwdi.dll') -Algorithm SHA256).Hash.ToLowerInvariant()
if ($manifest.build_script_sha256 -ne $scriptHash -or $manifest.dll_sha256 -ne $dllHash -or
    $manifest.source_sha256 -ne 'a695e93db0977dfdc5c6a99a4ea91b22f9027547d0177b2a0f3075078643c929') {
    throw 'libwdi DLL is stale or does not match the pinned source/build script. Re-run build-libwdi.ps1.'
}


if (-not $SkipBuild) {
    Push-Location $repo
    try {
        & cargo build --locked --release --target x86_64-pc-windows-gnu `
            -p directhci -p directhcid -p directhci-control-panel -p directhci-ble-cli
        if ($LASTEXITCODE -ne 0) { throw "Cargo build failed (exit $LASTEXITCODE)." }
    } finally {
        Pop-Location
    }
}

foreach ($binary in @('directhci.exe', 'directhcid.exe', 'directhci-control-panel.exe', 'directhci-ble.exe')) {
    $path = Join-Path $binaryDir $binary
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Missing $path. Build the Windows GNU release binaries first."
    }
}

$compiler = Get-Command ISCC.exe -ErrorAction SilentlyContinue
if ($compiler) {
    $iscc = $compiler.Source
} else {
    $candidates = @(
        (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'),
        (Join-Path $env:ProgramFiles 'Inno Setup 6\ISCC.exe')
    )
    $iscc = $candidates | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
}
if (-not $iscc) {
    throw 'Inno Setup 6 ISCC.exe is missing. Install Inno Setup 6 on the Windows build host; this script does not download tools.'
}

New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
$script = Join-Path $repo 'installer\directhci.iss'
& $iscc "/DDirectHciBinaryDir=$binaryDir" "/DDirectHciLibwdiDir=$libwdiDir" "/O$OutputDir" $script
if ($LASTEXITCODE -ne 0) { throw "Inno Setup compilation failed (exit $LASTEXITCODE)." }
Write-Host "Installer output: $OutputDir"
