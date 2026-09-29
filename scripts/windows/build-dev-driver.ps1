[CmdletBinding(DefaultParameterSetName = 'ExistingCertificate')]
param(
    [Parameter(ParameterSetName = 'ExistingCertificate', Mandatory = $true)]
    [ValidatePattern('^[0-9A-Fa-f ]+$')]
    [string] $CertificateThumbprint,

    [Parameter(ParameterSetName = 'CreateCertificate', Mandatory = $true)]
    [switch] $CreateCertificate,

    [string] $CertificateSubject = 'CN=DirectHCI Development Test',
    [string] $Inf2CatOs = '10_X64,10_CO_X64',

    [string] $OutputDirectory = (Join-Path $env:LOCALAPPDATA 'DirectHCI\driver\winusb-ax201-dev')
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repository = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$sourceInf = Join-Path $repository 'driver\winusb-ax201-dev\directhci-ax201-dev.inf'
$package = [System.IO.Path]::GetFullPath($OutputDirectory)
$inf = Join-Path $package 'directhci-ax201-dev.inf'
$catalog = Join-Path $package 'directhci-ax201-dev.cat'
$certificateFile = Join-Path $package 'directhci-development-test.cer'

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

if (-not (Test-Path $sourceInf)) {
    throw "source INF not found: $sourceInf"
}

New-Item -ItemType Directory -Path $package -Force | Out-Null
Copy-Item -LiteralPath $sourceInf -Destination $inf -Force

$infVerif = Find-WindowsKitTool 'InfVerif.exe'
$inf2Cat = Find-WindowsKitTool 'Inf2Cat.exe'
$signTool = Find-WindowsKitTool 'signtool.exe'

Write-Host "Validating $inf"
Invoke-Checked $infVerif @('/w', $inf)

if (Test-Path $catalog) {
    Remove-Item -LiteralPath $catalog -Force
}
Write-Host "Generating catalog for $Inf2CatOs"
Invoke-Checked $inf2Cat @("/driver:$package", "/os:$Inf2CatOs", '/verbose')

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
Write-Host "  Output:      $package"
Write-Host "  INF:         $inf"
Write-Host "  Catalog:     $catalog"
Write-Host "  Secure Boot: $secureBoot"
Write-Host "  Test signing: $testSigning"
Write-Host ''
Write-Warning 'This script did not trust a certificate, change boot policy, stage the package, or modify any device.'
Write-Host 'After manually satisfying the host signing policy, stage only with:'
Write-Host "  pnputil /add-driver `"$inf`""
Write-Host 'Do not use /install. Re-run `directhci takeover plan <id>` after staging.'
