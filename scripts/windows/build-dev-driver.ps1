[CmdletBinding(DefaultParameterSetName = 'ExistingCertificate')]
param(
    [Parameter(ParameterSetName = 'ExistingCertificate', Mandatory = $true)]
    [ValidatePattern('^[0-9A-Fa-f ]+$')]
    [string] $CertificateThumbprint,

    [Parameter(ParameterSetName = 'CreateCertificate', Mandatory = $true)]
    [switch] $CreateCertificate,

    [string] $CertificateSubject = 'CN=DirectHCI Development Test',
    [string] $Inf2CatOs = '10_X64,10_CO_X64',

    [string] $OutputDirectory
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repository = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$packageName = 'winusb-supported-devices'
$infName = 'directhci-winusb-dev.inf'
if (-not $OutputDirectory) {
    $OutputDirectory = Join-Path $env:LOCALAPPDATA "DirectHCI\driver\$packageName"
}
$templateInf = Join-Path $repository "driver\$packageName\directhci-winusb-dev.inf.in"
$supportedIds = Join-Path $repository "driver\$packageName\supported-hardware-ids.txt"
$packageDir = [System.IO.Path]::GetFullPath($OutputDirectory)
$inf = Join-Path $packageDir $infName
$catalog = Join-Path $packageDir ([System.IO.Path]::ChangeExtension($infName, '.cat'))
$certificateFile = Join-Path $packageDir 'directhci-development-test.cer'

function Find-WindowsKitTool {
    param([Parameter(Mandatory = $true)][string] $Name)

    $command = Get-Command $Name -ErrorAction SilentlyContinue
    if ($null -ne $command) {
        return $command.Source
    }

    $kitsBase = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10'
    foreach ($kitsRoot in @((Join-Path $kitsBase 'bin'), (Join-Path $kitsBase 'Tools'))) {
        if (Test-Path $kitsRoot) {
            $candidate = Get-ChildItem -Path $kitsRoot -Filter $Name -File -Recurse |
                Where-Object { $_.FullName -match '\\x64\\' } |
                Sort-Object FullName -Descending |
                Select-Object -First 1
            if ($null -ne $candidate) {
                return $candidate.FullName
            }
        }
    }

    throw "missing tool: $Name. Install the Windows Driver Kit/Windows SDK driver signing tools and run this script again."
}

function Invoke-Checked {
    param(
        [Parameter(Mandatory = $true)][string] $Program,
        [Parameter(Mandatory = $true)][string[]] $Arguments
    )

    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "tool failed with exit code $LASTEXITCODE`: $Program $($Arguments -join ' ')"
    }
}

if (-not (Test-Path -LiteralPath $templateInf -PathType Leaf) -or
    -not (Test-Path -LiteralPath $supportedIds -PathType Leaf)) {
    throw "Driver template or supported Hardware ID list not found in driver\$packageName."
}

$hardwareIds = [System.Collections.Generic.List[string]]::new()
$seenIds = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::OrdinalIgnoreCase)
foreach ($entry in (Get-Content -LiteralPath $supportedIds)) {
    $id = $entry.Trim()
    if ($id.Length -eq 0 -or $id.StartsWith('#')) { continue }
    if ($id -cnotmatch '^USB\\VID_[0-9A-Fa-f]{4}&PID_[0-9A-Fa-f]{4}(&MI_[0-9A-Fa-f]{2})?$') {
        throw ("Invalid PnP Hardware ID in {0}: {1}" -f $supportedIds, $id)
    }
    if (-not $seenIds.Add($id)) { throw "Duplicate PnP Hardware ID: $id" }
    $hardwareIds.Add($id.ToUpperInvariant())
}
if ($hardwareIds.Count -eq 0) { throw "Supported Hardware ID list is empty: $supportedIds" }
$sortedIds = $hardwareIds.ToArray()
[Array]::Sort($sortedIds, [System.StringComparer]::Ordinal)
$modelLines = @($sortedIds | ForEach-Object { '%DeviceName%=DirectHCI_Install,' + $_ })
$template = [System.IO.File]::ReadAllText($templateInf)
$marker = '; DIRECTHCI_MODELS'
if (-not $template.Contains($marker) -or
    $template.IndexOf($marker) -ne $template.LastIndexOf($marker)) {
    throw "INF template must contain exactly one model marker: $templateInf"
}
New-Item -ItemType Directory -Path $packageDir -Force | Out-Null
[System.IO.File]::WriteAllText($inf, $template.Replace($marker, ($modelLines -join [Environment]::NewLine)), [System.Text.Encoding]::ASCII)
Write-Host "Generated one INF with $($sortedIds.Length) explicit USB Hardware IDs."

$infVerif = Find-WindowsKitTool 'InfVerif.exe'
$inf2Cat = Find-WindowsKitTool 'Inf2Cat.exe'
$signTool = Find-WindowsKitTool 'signtool.exe'

Write-Host "Validating $inf"
Invoke-Checked $infVerif @('/w', $inf)

if (Test-Path $catalog) {
    Remove-Item -LiteralPath $catalog -Force
}
Write-Host "Generating catalog for $Inf2CatOs"
Invoke-Checked $inf2Cat @("/driver:$packageDir", "/os:$Inf2CatOs", '/verbose')

if ($CreateCertificate) {
    Write-Host 'Creating a current-user development signing certificate (not trusting it automatically)'
    $certificate = New-SelfSignedCertificate `
        -Type CodeSigningCert `
        -Subject $CertificateSubject `
        -CertStoreLocation 'Cert:\CurrentUser\My' `
        -KeyAlgorithm RSA `
        -KeyLength 3072 `
        -HashAlgorithm SHA256 `
        -KeyExportPolicy Exportable `
        -NotAfter (Get-Date).AddYears(2)
    $CertificateThumbprint = $certificate.Thumbprint
    Export-Certificate -Cert $certificate -FilePath $certificateFile -Force | Out-Null
    Write-Host "Public certificate exported to $certificateFile"
    Write-Warning 'The certificate is not trusted automatically. Trust/policy changes are manual user operations.'
}

$thumbprint = ($CertificateThumbprint -replace ' ', '').ToUpperInvariant()
Write-Host "Signing catalog with certificate $thumbprint"
Invoke-Checked $signTool @('sign', '/v', '/fd', 'SHA256', '/sha1', $thumbprint, $catalog)

Write-Host 'Verifying catalog signature and INF membership'
Invoke-Checked $signTool @('verify', '/v', '/pa', '/c', $catalog, $inf)

$secureBoot = 'unknown'
try {
    $secureBoot = [string](Confirm-SecureBootUEFI -ErrorAction Stop)
} catch {
    $secureBoot = "unknown ($($_.Exception.Message))"
}

$testSigning = 'unknown'
try {
    $bcd = (& bcdedit /enum '{current}' 2>&1 | Out-String)
    if ($LASTEXITCODE -eq 0) {
        $testSigning = if ($bcd -match '(?im)^testsigning\s+Yes\s*$') { 'enabled' } else { 'not reported as enabled' }
    }
} catch {
    $testSigning = "unknown ($($_.Exception.Message))"
}

Write-Host ''
Write-Host 'Development package complete.'
Write-Host "  Output:      $packageDir"
Write-Host "  INF:         $inf"
Write-Host "  Catalog:     $catalog"
Write-Host "  Secure Boot: $secureBoot"
Write-Host "  Test signing: $testSigning"
Write-Host ''
Write-Warning 'This script did not trust a certificate, change boot policy, stage the package, or modify any device.'
Write-Warning 'Listed Hardware IDs are candidates only; unverified devices require runtime topology and HCI validation. Review every matching radio and driver rank before staging.'
Write-Host 'After manually satisfying the host signing policy, stage only with:'
Write-Host "  pnputil /add-driver `"$inf`""
Write-Host 'Do not use /install. Re-run `directhci takeover plan <id>` after staging.'
