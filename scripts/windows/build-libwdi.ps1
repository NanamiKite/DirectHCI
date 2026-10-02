# Build-host-only source build. Run from the mounted EWDK's LaunchBuildEnv.cmd
# followed by SetupVSEnv. End-user hosts need neither EWDK nor MSBuild.
param(
    [string]$OutputDir = (Join-Path $env:LOCALAPPDATA 'DirectHCI\native'),
    [string]$EwdkRoot,
    [string]$PlatformToolset
)

$ErrorActionPreference = 'Stop'
$version = '1.5.1'
$expectedSha256 = 'a695e93db0977dfdc5c6a99a4ea91b22f9027547d0177b2a0f3075078643c929'
$work = Join-Path $env:LOCALAPPDATA 'DirectHCI\libwdi-build'
$archive = Join-Path $work "libwdi-$version.tar.gz"
$source = Join-Path $work "libwdi-$version"
$markerDir = Join-Path $work 'build-marker'

function Find-EwdkRoot([string]$Path) {
    if (-not $Path) { return $null }
    $directory = [System.IO.DirectoryInfo]::new($Path)
    while ($directory) {
        if (Test-Path -LiteralPath (Join-Path $directory.FullName 'LaunchBuildEnv.cmd') -PathType Leaf) {
            return $directory.FullName
        }
        $directory = $directory.Parent
    }
    return $null
}

if (-not $EwdkRoot) {
    $EwdkRoot = Find-EwdkRoot $env:VSINSTALLDIR
}
if (-not $EwdkRoot) {
    foreach ($drive in (Get-PSDrive -PSProvider FileSystem)) {
        if (Test-Path -LiteralPath (Join-Path $drive.Root 'LaunchBuildEnv.cmd') -PathType Leaf) {
            $EwdkRoot = $drive.Root
            break
        }
    }
}
if (-not $EwdkRoot -or -not (Test-Path -LiteralPath (Join-Path $EwdkRoot 'LaunchBuildEnv.cmd') -PathType Leaf)) {
    throw 'Mounted EWDK not found. Mount the EWDK ISO and run LaunchBuildEnv.cmd, then SetupVSEnv, before invoking this script.'
}
$msbuild = Get-Command MSBuild.exe -ErrorAction SilentlyContinue
$compiler = Get-Command cl.exe -ErrorAction SilentlyContinue
if (-not $msbuild -or -not $compiler -or -not $env:VCToolsInstallDir) {
    throw "EWDK MSVC environment is not active. In a cmd.exe window run '$(Join-Path $EwdkRoot 'LaunchBuildEnv.cmd')', then 'SetupVSEnv', then invoke this PowerShell script from that window."
}
$ewdkPrefix = [System.IO.Path]::GetFullPath($EwdkRoot).TrimEnd('\') + '\'
if (-not $msbuild.Source.StartsWith($ewdkPrefix, [System.StringComparison]::OrdinalIgnoreCase) -or
    -not $compiler.Source.StartsWith($ewdkPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "MSBuild or cl.exe is not from the mounted EWDK at $EwdkRoot. Use its LaunchBuildEnv.cmd and SetupVSEnv environment."
}
if (-not $PlatformToolset) {
    if ($env:VCToolsVersion -notmatch '^14\.(\d{2})') {
        throw 'EWDK VCToolsVersion is missing or unrecognized; pass -PlatformToolset explicitly.'
    }
    $PlatformToolset = 'v14' + [int][math]::Floor([int]$Matches[1] / 10)
}
New-Item -ItemType Directory -Force -Path $work, $OutputDir, $markerDir | Out-Null
if (-not (Test-Path -LiteralPath $archive -PathType Leaf)) {
    Invoke-WebRequest -Uri "https://codeload.github.com/pbatard/libwdi/tar.gz/refs/tags/v$version" -OutFile $archive
}
$hash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
if ($hash -ne $expectedSha256) {
    throw "libwdi source archive SHA-256 mismatch: $hash"
}
if (-not (Test-Path -LiteralPath $source -PathType Container)) {
    & tar.exe -xf $archive -C $work
    if ($LASTEXITCODE -ne 0) { throw "Extract libwdi source failed: $LASTEXITCODE" }
}
# Pinned libwdi 1.5.1 reports successful signing even if deletion of its
# temporary trusted private key fails. Fail closed and remove the public trust
# entries on any signing/key-destruction failure. The source archive plus this
# transformation must be distributed alongside the LGPL DLL if shipped.
$pkiPath = Join-Path $source 'libwdi\pki.c'
$pki = [System.IO.File]::ReadAllText($pkiPath).Replace("`r`n", "`n")
$oldDelete = "`twdi_warn(`"Failed to delete private key: %s`", winpki_error_str(0));`n`t}"
$newDelete = "`twdi_warn(`"Failed to delete private key: %s`", winpki_error_str(0));`n`t`tgoto out;`n`t}"
if ($pki.Contains($oldDelete)) { $pki = $pki.Replace($oldDelete, $newDelete) }
$oldCleanup = @'
if ((pCertContext != NULL) && (DeletePrivateKey(pCertContext))) {
		wdi_info("Successfully deleted private key");
	}
'@
$newCleanup = @'
if (pCertContext != NULL) {
		if (!DeletePrivateKey(pCertContext)) {
			wdi_warn("Private key could not be destroyed; signed package is unusable");
			r = FALSE;
		} else {
			wdi_info("Successfully deleted private key");
		}
		if (!r) {
			RemoveCertFromStore(szCertSubject, "Root");
			RemoveCertFromStore(szCertSubject, "TrustedPublisher");
		}
	}
'@
if ($pki.Contains($oldCleanup)) { $pki = $pki.Replace($oldCleanup, $newCleanup) }
if (-not $pki.Contains('Private key could not be destroyed; signed package is unusable') -or
    -not $pki.Contains("goto out;`n`t}`n`n`t// This is optional")) {
    throw 'Pinned libwdi private-key cleanup patch did not apply; refusing to build.'
}
[System.IO.File]::WriteAllText($pkiPath, $pki)

# The external INF uses in-box WinUSB: no legacy WDK co-installers are needed.
# Keep this narrow exception out of ordinary libwdi-generated WinUSB packages.
$wdiPath = Join-Path $source 'libwdi\libwdi.c'
$wdi = [System.IO.File]::ReadAllText($wdiPath).Replace("`r`n", "`n")
$oldSupport = 'if (!wdi_is_driver_supported(driver_type, &driver_version[driver_type])) {'
$newSupport = 'if (!((options != NULL) && options->external_inf && driver_type == WDI_WINUSB) && !wdi_is_driver_supported(driver_type, &driver_version[driver_type])) {'
if ($wdi.Contains($oldSupport)) { $wdi = $wdi.Replace($oldSupport, $newSupport) }
$oldCatalog = 'wdi_tokenize_resource(cat_template[driver_type],'
$newCatalog = 'wdi_tokenize_resource(cat_template[((options != NULL) && options->external_inf && driver_type == WDI_WINUSB)?WDI_CDC:driver_type],'
if ($wdi.Contains($oldCatalog)) { $wdi = $wdi.Replace($oldCatalog, $newCatalog) }
if (-not $wdi.Contains($newSupport) -or -not $wdi.Contains($newCatalog)) {
    throw 'Pinned libwdi external WinUSB INF/catalog patch did not apply; refusing to build.'
}
[System.IO.File]::WriteAllText($wdiPath, $wdi)

# libwdi needs at least one embedded user file when built without old WDK
# co-installers. The runtime uses its no-extra-binary catalog path with an
# external WinUSB INF; this marker is never referenced by the INF or CAT.
$marker = Join-Path $markerDir 'directhci-libwdi-build-marker.txt'
[System.IO.File]::WriteAllText($marker, "DirectHCI libwdi build marker`n")
# The official MSVC config defaults to WDK 8 and libusb redistributables,
# none of which are part of DirectHCI's external, in-box WinUSB INF. Use only
# the existing marker as USER_DIR and the x64 installer helper resource.
$configPath = Join-Path $source 'msvc\config.h'
$config = [System.IO.File]::ReadAllText($configPath).Replace("`r`n", "`n")
$disabled = @(
    '#define WDK_DIR "C:/Program Files (x86)/Windows Kits/8.0"',
    '#define LIBUSB0_DIR "D:/libusb-win32"',
    '#define LIBUSBK_DIR "D:/libusbK/bin"',
    '#define OPT_M32',
    '#define OPT_ARM'
)
foreach ($definition in $disabled) {
    $original = '(?m)^' + [regex]::Escape($definition) + '$'
    $patched = '// DirectHCI build: ' + $definition
    if ([regex]::IsMatch($config, $original)) {
        $config = [regex]::Replace($config, $original, $patched)
    }
    if (-not $config.Contains($patched) -or [regex]::IsMatch($config, $original)) {
        throw "Pinned libwdi MSVC config patch did not apply: $definition"
    }
}
$markerPath = $markerDir.Replace('\', '/')
$userDefinition = "#define USER_DIR `"$markerPath`""
if ($config.Contains('// #define USER_DIR "C:/signed-driver"')) {
    $config = $config.Replace('// #define USER_DIR "C:/signed-driver"', $userDefinition)
}
if (-not $config.Contains($userDefinition) -or -not $config.Contains('#define OPT_M64')) {
    throw 'Pinned libwdi MSVC resource config patch did not apply; refusing to build.'
}
[System.IO.File]::WriteAllText($configPath, $config)

# The official Release DLL uses /MD, but Setup does not install a VC++ redist.
# Use /MT for this x64 build so end-user machines only need DirectHCI-Setup.
$dllProjectPath = Join-Path $source 'libwdi\.msvc\libwdi_dll.vcxproj'
$dllProject = [System.IO.File]::ReadAllText($dllProjectPath)
$releaseCondition = '<ItemDefinitionGroup Condition="''$(Configuration)|$(Platform)''==''Release|x64''">'
$releaseStart = $dllProject.IndexOf($releaseCondition, [System.StringComparison]::Ordinal)
if ($releaseStart -lt 0) { throw 'Pinned libwdi Release|x64 MSVC configuration was not found.' }
$releaseEnd = $dllProject.IndexOf('</ItemDefinitionGroup>', $releaseStart, [System.StringComparison]::Ordinal)
if ($releaseEnd -lt 0) { throw 'Pinned libwdi Release|x64 MSVC configuration is malformed.' }
$releaseBlock = $dllProject.Substring($releaseStart, $releaseEnd - $releaseStart)
if ($releaseBlock.Contains('<RuntimeLibrary>MultiThreadedDLL</RuntimeLibrary>')) {
    $staticBlock = $releaseBlock.Replace('<RuntimeLibrary>MultiThreadedDLL</RuntimeLibrary>',
        '<RuntimeLibrary>MultiThreaded</RuntimeLibrary>')
    $dllProject = $dllProject.Substring(0, $releaseStart) + $staticBlock + $dllProject.Substring($releaseEnd)
} elseif (-not $releaseBlock.Contains('<RuntimeLibrary>MultiThreaded</RuntimeLibrary>')) {
    throw 'Pinned libwdi x64 runtime library setting changed; refusing to build.'
}
# The official DLL project references x86 and ARM64 helpers even for x64.
# We build only the x64 resource dependencies below, then compile the same
# official DLL project without those cross-architecture reference edges.
$referenceMarker = '<!-- DirectHCI explicitly builds the x64 resource dependencies -->'
$firstReference = $dllProject.IndexOf('<ProjectReference Include="embedder.vcxproj">', [System.StringComparison]::Ordinal)
if ($firstReference -ge 0) {
    $groupStart = $dllProject.LastIndexOf('<ItemGroup>', $firstReference, [System.StringComparison]::Ordinal)
    $groupEnd = $dllProject.IndexOf('</ItemGroup>', $firstReference, [System.StringComparison]::Ordinal)
    if ($groupStart -lt 0 -or $groupEnd -lt 0) { throw 'Pinned libwdi DLL project reference group is malformed.' }
    $groupEnd += '</ItemGroup>'.Length
    $referenceGroup = $dllProject.Substring($groupStart, $groupEnd - $groupStart)
    foreach ($name in @('embedder', 'installer_arm64', 'installer_x64', 'installer_x86')) {
        if (-not $referenceGroup.Contains("<ProjectReference Include=`"$name.vcxproj`">")) {
            throw "Pinned libwdi DLL project reference $name changed; refusing to build."
        }
    }
    if ([regex]::Matches($referenceGroup, '<ProjectReference Include=').Count -ne 4) {
        throw 'Pinned libwdi DLL project has unexpected additional references; refusing to build.'
    }
    $dllProject = $dllProject.Substring(0, $groupStart) + $referenceMarker + $dllProject.Substring($groupEnd)
} elseif (-not $dllProject.Contains($referenceMarker)) {
    throw 'Pinned libwdi DLL project references changed; refusing to build.'
}
[System.IO.File]::WriteAllText($dllProjectPath, $dllProject)

# Build only the official Visual Studio projects needed for the x64 DLL.
# Project references are built explicitly: embedder is Win32-only, while the
# installer helper and final DLL are x64. This avoids unrelated Zadig/examples.
function Invoke-LibwdiMsBuild([string]$Project, [string]$Platform) {
    $projectPath = Join-Path $source "libwdi\.msvc\$Project.vcxproj"
    if (-not (Test-Path -LiteralPath $projectPath -PathType Leaf)) {
        throw "Pinned libwdi MSVC project is missing: $projectPath"
    }
    Write-Host "MSBuild $Project (Release|$Platform, $PlatformToolset)"
    $solutionDir = $source.Replace('\', '/') + '/'
    & $msbuild.Source $projectPath /nologo /m:1 /t:Build `
        '/p:Configuration=Release' "/p:Platform=$Platform" `
        "/p:PlatformToolset=$PlatformToolset" "/p:SolutionDir=$solutionDir" `
        '/p:BuildProjectReferences=false'
    if ($LASTEXITCODE -ne 0) {
        throw "Official libwdi MSVC project $Project failed with exit code $LASTEXITCODE. Check that the mounted EWDK includes the selected $PlatformToolset desktop C++ and Windows SDK tools; no MSYS2 fallback is used."
    }
}
Invoke-LibwdiMsBuild 'installer_x64' 'x64'
Invoke-LibwdiMsBuild 'detect_64build' 'Win32'
Invoke-LibwdiMsBuild 'embedder' 'Win32'
Invoke-LibwdiMsBuild 'libwdi_dll' 'x64'
$dllPath = Join-Path $source 'x64\Release\dll\libwdi.dll'
if (-not (Test-Path -LiteralPath $dllPath -PathType Leaf)) {
    throw "Official libwdi MSVC DLL output was not found: $dllPath"
}
$dll = Get-Item -LiteralPath $dllPath
$outputDll = Join-Path $OutputDir 'libwdi.dll'
Copy-Item -LiteralPath $dll.FullName -Destination $outputDll -Force
Copy-Item -LiteralPath $archive -Destination (Join-Path $OutputDir "libwdi-$version-source.tar.gz") -Force
@{
    source_sha256 = $expectedSha256
    build_script_sha256 = (Get-FileHash -LiteralPath $PSCommandPath -Algorithm SHA256).Hash.ToLowerInvariant()
    dll_sha256 = (Get-FileHash -LiteralPath $outputDll -Algorithm SHA256).Hash.ToLowerInvariant()
} | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $OutputDir 'libwdi-build.json') -Encoding UTF8
Write-Host "libwdi DLL: $outputDll"
Write-Host "LGPL source: $(Join-Path $OutputDir "libwdi-$version-source.tar.gz")"
