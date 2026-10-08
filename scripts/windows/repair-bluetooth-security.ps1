# Incident repair only: clear the known DirectHCI device-security override
# on a healthy, already Windows-owned USB Bluetooth controller. No rebind.
[CmdletBinding(SupportsShouldProcess = $true, ConfirmImpact = 'High')]
param(
    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string] $InstanceId,
    [switch] $Apply
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run this script from an Administrator PowerShell window.'
}
if ($InstanceId -notmatch '^USB\\VID_[0-9A-F]{4}&PID_[0-9A-F]{4}[^\\]*\\[^\\]+$') {
    throw 'An exact USB device Instance ID is required; wildcards are not allowed.'
}

function Assert-Idle {
    $service = Get-Service -Name DirectHCI -ErrorAction SilentlyContinue
    if ($null -ne $service -and $service.Status -ne 'Stopped') {
        throw 'Stop DirectHCI and close its consumers before repair.'
    }
    if (@(Get-Process -Name directhcid -ErrorAction SilentlyContinue).Count -ne 0) {
        throw 'A directhcid process is still running; no device property was changed.'
    }
    $journal = Join-Path ([Environment]::GetFolderPath('CommonApplicationData')) 'DirectHCI\ownership-journal-v1.json'
    if (Test-Path -LiteralPath $journal -ErrorAction Stop) {
        throw 'An ownership journal exists. Resolve it before this incident repair; do not delete it.'
    }
}

function Get-RequiredControllerProperty([string] $Name) {
    $key = 'DEVPKEY_Device_' + $Name
    try {
        $result = @(Get-PnpDeviceProperty -InstanceId $InstanceId -KeyName $key -ErrorAction Stop)
    } catch {
        throw "Cannot read required device property ${key}: $($_.Exception.Message). No device property was changed."
    }
    if ($result.Count -ne 1 -or $null -eq $result[0]) {
        throw "Expected one value for required device property ${key}; repair refused."
    }
    # Some CIM property objects have no Data member. Do not let StrictMode
    # obscure which required identity/health property could not be read.
    $data = $result[0].PSObject.Properties['Data']
    if ($null -eq $data -or $null -eq $data.Value -or
        [string]::IsNullOrWhiteSpace([string]$data.Value)) {
        throw "Required device property ${key} has no usable Data; repair refused."
    }
    return $data.Value
}

function Get-WindowsController {
    $device = Get-PnpDevice -InstanceId $InstanceId -PresentOnly
    if ($device.Status -ne 'OK') {
        throw 'The selected controller is not healthy; refusing device-security repair.'
    }
    $properties = @{}
    foreach ($key in @('ClassGuid', 'Service', 'ProblemCode', 'DriverInfPath', 'DriverProvider')) {
        $properties[$key] = Get-RequiredControllerProperty -Name $key
    }
    if ([guid]$properties.ClassGuid -ne [guid]'e0cbf06c-cd8b-4647-bb8a-263b43f0f974' -or
        $properties.Service -notin @('BTHUSB', 'IBTUSB') -or
        $null -eq $properties.ProblemCode -or [uint32]$properties.ProblemCode -ne 0 -or
        [string]::IsNullOrWhiteSpace([string]$properties.DriverInfPath) -or
        [string]::IsNullOrWhiteSpace([string]$properties.DriverProvider) -or
        [string]$properties.DriverProvider -match 'DirectHCI') {
        throw 'The selected device must already have a healthy Windows Bluetooth binding.'
    }
    # SecuritySDS is optional and its CIM wrapper need not expose Data.
    # Read it through SetupAPI, distinguishing absence from other API errors.
    $properties['SecuritySDS'] = [DirectHciIncidentSecurityV2]::ReadSecuritySddl($InstanceId)
    if ($null -ne $properties.SecuritySDS -and
        [string]$properties.SecuritySDS -cne 'D:P(A;;GA;;;SY)(A;;GA;;;BA)') {
        throw 'The security descriptor is not the exact known DirectHCI override; refusing to replace another policy.'
    }
    return $properties
}

Assert-Idle
# Use a new type name so rerunning this script in a PowerShell window that
# previously loaded the old helper does not reuse that cached implementation.
if (-not ('DirectHciIncidentSecurityV2' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;

public static class DirectHciIncidentSecurityV2 {
    [StructLayout(LayoutKind.Sequential)]
    private struct DeviceInfo {
        public UInt32 Size;
        public Guid ClassGuid;
        public UInt32 DevInst;
        public UIntPtr Reserved;
    }
    [DllImport("setupapi.dll", SetLastError = true)]
    private static extern IntPtr SetupDiCreateDeviceInfoList(ref Guid classGuid, IntPtr parent);
    [DllImport("setupapi.dll", CharSet = CharSet.Unicode, ExactSpelling = true, SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool SetupDiOpenDeviceInfoW(IntPtr set, string id, IntPtr parent, UInt32 flags, ref DeviceInfo info);
    [DllImport("setupapi.dll", CharSet = CharSet.Unicode, ExactSpelling = true, SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool SetupDiGetDeviceRegistryPropertyW(IntPtr set, ref DeviceInfo info, UInt32 property, out UInt32 type, byte[] data, UInt32 size, out UInt32 needed);
    [DllImport("setupapi.dll", CharSet = CharSet.Unicode, ExactSpelling = true, SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool SetupDiSetDeviceRegistryPropertyW(IntPtr set, ref DeviceInfo info, UInt32 property, IntPtr data, UInt32 size);
    [DllImport("setupapi.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool SetupDiDestroyDeviceInfoList(IntPtr set);

    private static string ReadString(IntPtr set, ref DeviceInfo info, UInt32 property, bool optional = false) {
        UInt32 type, needed;
        byte[] bytes = new byte[65536];
        if (!SetupDiGetDeviceRegistryPropertyW(set, ref info, property, out type, bytes, (UInt32)bytes.Length, out needed)) {
            int error = Marshal.GetLastWin32Error();
            // SetupAPI reports an absent property as ERROR_INVALID_DATA or
            // ERROR_NOT_FOUND. Access denied and all other errors must fail.
            if (optional && (error == 13 || error == 1168)) return null;
            throw new Win32Exception(error, "Read device property " + property + " (Win32 " + error + ")");
        }
        if (type != 1 || needed < 2 || needed > bytes.Length || needed % 2 != 0)
            throw new InvalidOperationException("Unexpected device property encoding");
        if (bytes[needed - 1] != 0 || bytes[needed - 2] != 0)
            throw new InvalidOperationException("Device property is not null-terminated");
        return Encoding.Unicode.GetString(bytes, 0, (int)needed).TrimEnd('\0');
    }

    public static string ReadSecuritySddl(string id) {
        Guid bluetooth = new Guid("e0cbf06c-cd8b-4647-bb8a-263b43f0f974");
        IntPtr set = SetupDiCreateDeviceInfoList(ref bluetooth, IntPtr.Zero);
        if (set == new IntPtr(-1)) throw new Win32Exception(Marshal.GetLastWin32Error(), "Create device information set");
        try {
            DeviceInfo info = new DeviceInfo();
            info.Size = (UInt32)Marshal.SizeOf(typeof(DeviceInfo));
            if (!SetupDiOpenDeviceInfoW(set, id, IntPtr.Zero, 0, ref info))
                throw new Win32Exception(Marshal.GetLastWin32Error(), "Open exact Bluetooth device");
            return ReadString(set, ref info, 24, true); // SPDRP_SECURITY_SDS
        } finally {
            SetupDiDestroyDeviceInfoList(set);
        }
    }

    public static void ClearKnownOverride(string id) {
        Guid bluetooth = new Guid("e0cbf06c-cd8b-4647-bb8a-263b43f0f974");
        IntPtr set = SetupDiCreateDeviceInfoList(ref bluetooth, IntPtr.Zero);
        if (set == new IntPtr(-1)) throw new Win32Exception(Marshal.GetLastWin32Error(), "Create device information set");
        try {
            DeviceInfo info = new DeviceInfo();
            info.Size = (UInt32)Marshal.SizeOf(typeof(DeviceInfo));
            if (!SetupDiOpenDeviceInfoW(set, id, IntPtr.Zero, 0, ref info))
                throw new Win32Exception(Marshal.GetLastWin32Error(), "Open exact Bluetooth device");
            string service = ReadString(set, ref info, 4); // SPDRP_SERVICE
            if (!String.Equals(service, "BTHUSB", StringComparison.OrdinalIgnoreCase) &&
                !String.Equals(service, "IBTUSB", StringComparison.OrdinalIgnoreCase))
                throw new InvalidOperationException("Windows Bluetooth binding changed; refusing repair");
            if (ReadString(set, ref info, 24) != "D:P(A;;GA;;;SY)(A;;GA;;;BA)") // SPDRP_SECURITY_SDS
                throw new InvalidOperationException("Device security changed; refusing repair");
            // NULL + zero clears the device-specific override. Never install a
            // guessed descriptor or a NULL DACL; class/driver policy remains.
            if (!SetupDiSetDeviceRegistryPropertyW(set, ref info, 23, IntPtr.Zero, 0)) // SPDRP_SECURITY
                throw new Win32Exception(Marshal.GetLastWin32Error(), "Clear device security override");
            if (ReadString(set, ref info, 24, true) != null)
                throw new InvalidOperationException("Device security override is still present after clearing; stop and inspect the saved backup");
        } finally {
            SetupDiDestroyDeviceInfoList(set);
        }
    }
}
'@
}

Assert-Idle
$before = Get-WindowsController
Write-Host "Controller: $InstanceId"
Write-Host "Driver:     $($before.Service) / $($before.DriverInfPath) / $($before.DriverProvider)"
if ($null -eq $before.SecuritySDS) {
    Write-Host 'No explicit device security override was reported by SetupAPI. Nothing was changed; the controller was not restarted.'
    Write-Host 'This does not verify effective device-object permissions or Windows Bluetooth functionality.'
    return
}
Write-Host "Override:   $($before.SecuritySDS)"
if (-not $Apply) {
    Write-Host 'Preview only. Use -Apply to back up and clear this one device override, then restart this controller.'
    return
}
if (-not $PSCmdlet.ShouldProcess($InstanceId, 'Back up and clear the known DirectHCI security override, then restart this Bluetooth controller')) {
    return
}

Assert-Idle
$fresh = Get-WindowsController
if ($null -eq $fresh.SecuritySDS) {
    Write-Host 'The override is no longer present. Nothing was changed; the controller was not restarted.'
    return
}
if ($fresh.DriverInfPath -ne $before.DriverInfPath -or $fresh.DriverProvider -ne $before.DriverProvider) {
    throw 'The Windows driver changed during repair preparation; no device property was changed.'
}
$backupRoot = Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'DirectHCI\recovery-backups'
New-Item -ItemType Directory -Path $backupRoot -Force | Out-Null
$backupPath = Join-Path $backupRoot ('device-security-' + [guid]::NewGuid().ToString('N') + '.json')
$backup = [ordered]@{
    purpose = 'Observed restrictive override backup, NOT a pre-DirectHCI baseline'
    observed_utc = [DateTime]::UtcNow.ToString('o')
    instance_id = $InstanceId
    service = $fresh.Service
    driver_inf = $fresh.DriverInfPath
    driver_provider = $fresh.DriverProvider
    observed_security_sddl = $fresh.SecuritySDS
} | ConvertTo-Json
$stream = [IO.File]::Open($backupPath, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
try {
    $bytes = [Text.Encoding]::UTF8.GetBytes($backup)
    $stream.Write($bytes, 0, $bytes.Length)
    $stream.Flush($true)
} finally {
    $stream.Dispose()
}
Write-Host "Backup: $backupPath"
Assert-Idle
[DirectHciIncidentSecurityV2]::ClearKnownOverride($InstanceId)
Write-Host 'Device-specific override cleared. Restarting only this Bluetooth controller.'
$pnputil = Join-Path ([Environment]::GetFolderPath('System')) 'pnputil.exe'
& $pnputil /restart-device $InstanceId
$restartCode = $LASTEXITCODE
if ($restartCode -eq 3010) {
    Write-Warning 'Windows requests a reboot. Leave DirectHCI stopped, reboot, then check Windows Bluetooth.'
    return
}
if ($restartCode -ne 0) {
    throw "Override was cleared, but the controller restart failed (exit $restartCode). Backup: $backupPath. Do not start DirectHCI; save the error output."
}
Write-Host 'Restart request completed. Inspect the controller and try enabling Bluetooth in Windows Settings.'
Write-Host 'This does not prove Bluetooth is functional. Leave DirectHCI stopped until Windows Bluetooth is verified.'
Get-PnpDevice -InstanceId $InstanceId -PresentOnly | Format-List Status, FriendlyName, InstanceId
Get-PnpDeviceProperty -InstanceId $InstanceId -KeyName 'DEVPKEY_Device_Service', 'DEVPKEY_Device_DriverInfPath', 'DEVPKEY_Device_ProblemCode' | Format-List KeyName, Data
