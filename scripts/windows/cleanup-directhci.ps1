# One-off, guarded cleanup of legacy DirectHCI development residue.
# Full installation/state removal requires the additional -FullUninstall switch.
# -ForceUninstall additionally authorizes archiving a pending recovery journal
# after current Windows binding checks, without certifying reboot/radio recovery.
# Windows-profile build trees and signing keys require separate explicit switches.
# Shared/source repository directories are never cleanup targets.
# Run in 64-bit Administrator Windows PowerShell. Preview is the default.
[CmdletBinding()]
param(
    [switch] $Apply,
    [switch] $FullUninstall,
    [switch] $ReconcileWindowsOwned,
    [switch] $ForceUninstall,
    [switch] $RepairLegacySecurity,
    [switch] $RemoveDevelopmentArtifacts,
    [switch] $RemoveDevelopmentSigningKeys
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (-not [Environment]::Is64BitProcess -or $env:OS -ne 'Windows_NT') {
    throw 'Use 64-bit Windows PowerShell on the Windows host.'
}
$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Open Windows PowerShell as Administrator.'
}
if (($RemoveDevelopmentArtifacts -or $RemoveDevelopmentSigningKeys) -and -not $FullUninstall) {
    throw 'Development artifact/key removal requires -FullUninstall. Preview remains the default.'
}
if ($ReconcileWindowsOwned -and -not $FullUninstall) {
    throw 'Completed-recovery reconciliation requires -FullUninstall; it is not a takeover/recovery bypass.'
}
if ($ForceUninstall -and (-not $FullUninstall -or $ReconcileWindowsOwned -or $RepairLegacySecurity)) {
    throw '-ForceUninstall requires -FullUninstall and cannot be combined with -ReconcileWindowsOwned or -RepairLegacySecurity. It removes an installation without certifying reboot/radio recovery.'
}

$installDir = Join-Path ([Environment]::GetFolderPath('ProgramFiles')) 'DirectHCI'
$stateDir = Join-Path ([Environment]::GetFolderPath('CommonApplicationData')) 'DirectHCI'
$journal = Join-Path $stateDir 'ownership-journal-v1.json'
$cli = Join-Path $installDir 'directhci.exe'
$daemon = Join-Path $installDir 'directhcid.exe'
$pnputil = Join-Path ([Environment]::GetFolderPath('System')) 'pnputil.exe'
$guid = '{CF97AABE-7898-4D73-B044-A481B26747AA}'
$appKey = '{C6F4C08B-6961-49FE-98AB-6CEA79EC0D6E}_is1'
$uninstallKey = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\' + $appKey
$serviceKey = 'HKLM:\SYSTEM\CurrentControlSet\Services\DirectHCI'
$eventSourceKey = 'HKLM:\SYSTEM\CurrentControlSet\Services\EventLog\Application\DirectHCI'
$backup = $null

function Assert-PlainTree([string] $Path) {
    # Do not traverse a junction/symlink before inspecting it (including parents).
    $existing = [IO.Path]::GetFullPath($Path)
    while (-not (Test-Path -LiteralPath $existing)) {
        $ancestor = [IO.Directory]::GetParent($existing)
        if ($null -eq $ancestor) { throw "Cannot inspect path ancestry: $Path" }
        $existing = $ancestor.FullName
    }
    $parent = Get-Item -LiteralPath $existing -Force
    if (-not $parent.PSIsContainer) { throw "Expected a directory: $existing" }
    while ($null -ne $parent) {
        if (($parent.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Reparse point refused: $($parent.FullName)"
        }
        $parent = $parent.Parent
    }
    if (-not (Test-Path -LiteralPath $Path)) { return }
    $pending = [Collections.Generic.Queue[string]]::new()
    $pending.Enqueue($Path)
    while ($pending.Count -gt 0) {
        $directory = $pending.Dequeue()
        foreach ($item in Get-ChildItem -LiteralPath $directory -Force) {
            if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "Reparse point refused: $($item.FullName)"
            }
            if ($item.PSIsContainer) { $pending.Enqueue($item.FullName) }
        }
    }
}

function Get-DriverInventory {
    # Structured DISM output: do not parse localized pnputil text.
    return @(Get-WindowsDriver -Online)
}

function Get-OwnedDrivers($Drivers) {
    foreach ($driver in $Drivers) {
        if ($driver.ProviderName -cne 'DirectHCI Project') { continue }
        if (-not $FullUninstall -and [IO.Path]::GetFileName($driver.OriginalFileName) -notin @(
            'directhci-ax201-dev.inf', 'directhci-bluetooth-class-dev.inf', 'directhci-winusb-dev.inf')) {
            continue
        }
        if ($driver.Driver -notmatch '^oem[0-9]+\.inf$') {
            throw "Unexpected published INF name: $($driver.Driver)"
        }
        $inf = [IO.Path]::GetFullPath($driver.OriginalFileName)
        $store = Join-Path $env:windir 'System32\DriverStore\FileRepository'
        if (-not $inf.StartsWith($store + '\', [StringComparison]::OrdinalIgnoreCase)) {
            throw "Unexpected Driver Store path: $inf"
        }
        $text = [IO.File]::ReadAllText($inf)
        if (-not $text.Contains($guid) -or $text -notmatch '(?im)^\s*Include\s*=\s*winusb\.inf\s*$') {
            throw "DirectHCI-labelled package has an unfamiliar interface/install section: $inf"
        }
        $driver
    }
}

function Get-RequiredPnpData([string] $InstanceId, [string] $KeyName) {
    try {
        $items = @(Get-PnpDeviceProperty -InstanceId $InstanceId -KeyName $KeyName -ErrorAction Stop)
    } catch {
        throw "Cannot read ${KeyName} for ${InstanceId}: $($_.Exception.Message); cleanup stopped."
    }
    if ($items.Count -ne 1 -or $null -eq $items[0]) {
        throw "Expected one ${KeyName} value for ${InstanceId}; cleanup stopped."
    }
    $data = $items[0].PSObject.Properties['Data']
    if ($null -eq $data -or $null -eq $data.Value -or
        [string]::IsNullOrWhiteSpace([string]$data.Value)) {
        throw "Required ${KeyName} has no usable Data for ${InstanceId}; cleanup stopped."
    }
    return $data.Value
}

function Get-DeviceSecurityOverride([string] $InstanceId) {
    # The optional SecuritySDS CIM property need not have a Data member.
    # Read its actual value through SetupAPI instead of treating an unfamiliar
    # CIM wrapper as an absent security policy. This helper never writes.
    if (-not ('DirectHciCleanupSecurityReaderV1' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;
public static class DirectHciCleanupSecurityReaderV1 {
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
    private static extern bool SetupDiGetDeviceRegistryPropertyW(IntPtr set, ref DeviceInfo info, UInt32 property, out UInt32 type, [Out] byte[] data, UInt32 size, out UInt32 needed);
    [DllImport("setupapi.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool SetupDiDestroyDeviceInfoList(IntPtr set);
    public static string Read(string id) {
        Guid bluetooth = new Guid("e0cbf06c-cd8b-4647-bb8a-263b43f0f974");
        IntPtr set = SetupDiCreateDeviceInfoList(ref bluetooth, IntPtr.Zero);
        if (set == new IntPtr(-1)) throw new Win32Exception(Marshal.GetLastWin32Error());
        try {
            DeviceInfo info = new DeviceInfo();
            info.Size = (UInt32)Marshal.SizeOf(typeof(DeviceInfo));
            if (!SetupDiOpenDeviceInfoW(set, id, IntPtr.Zero, 0, ref info))
                throw new Win32Exception(Marshal.GetLastWin32Error(), "Open exact Bluetooth device for security inspection");
            byte[] bytes = new byte[65536];
            UInt32 type, needed;
            if (!SetupDiGetDeviceRegistryPropertyW(set, ref info, 24, out type, bytes, (UInt32)bytes.Length, out needed)) {
                int error = Marshal.GetLastWin32Error();
                // Absence/invalid property data is reported as 13; 1168 is
                // ERROR_NOT_FOUND. This read is not a radio-health check.
                if (error == 13 || error == 1168) return null;
                throw new Win32Exception(error, "Read device security (Win32 " + error + ")");
            }
            if (type != 1 || needed < 2 || needed > bytes.Length || needed % 2 != 0 ||
                bytes[needed - 1] != 0 || bytes[needed - 2] != 0)
                throw new InvalidOperationException("Unexpected device security encoding; cleanup stopped");
            return Encoding.Unicode.GetString(bytes, 0, (int)needed - 2);
        } finally { SetupDiDestroyDeviceInfoList(set); }
    }
}
'@
    }
    return [DirectHciCleanupSecurityReaderV1]::Read($InstanceId)
}

function Get-ControllerState {
    if (Test-Path -LiteralPath $cli -PathType Leaf) {
        $output = @(& $cli controllers --direct --json)
        if ($LASTEXITCODE -ne 0) { throw 'Controller enumeration failed; no cleanup.' }
        return @((($output -join [Environment]::NewLine) | ConvertFrom-Json))
    }
    # After a successful uninstall, use native PnP only for verification.
    foreach ($device in Get-PnpDevice -Class Bluetooth -PresentOnly) {
        if ($device.InstanceId -notmatch '^USB\\') { continue }
        $values = @{}
        # Only request the fields needed for ownership verification. Unrelated
        # optional CIM properties may legitimately lack a Data member.
        foreach ($key in @('Service', 'DriverInfPath', 'DriverProvider', 'ProblemCode')) {
            $name = 'DEVPKEY_Device_' + $key
            $values[$name] = Get-RequiredPnpData -InstanceId $device.InstanceId -KeyName $name
        }
        [pscustomobject]@{
            identity = [pscustomobject]@{ instance_id = $device.InstanceId }
            service = $values['DEVPKEY_Device_Service']
            driver = [pscustomobject]@{
                inf_path = $values['DEVPKEY_Device_DriverInfPath']
                provider = $values['DEVPKEY_Device_DriverProvider']
            }
            status = [pscustomobject]@{
                present = $true
                problem_code = $values['DEVPKEY_Device_ProblemCode']
            }
        }
    }
}

function Assert-WindowsOwned($Controllers) {
    foreach ($controller in $Controllers) {
        if (-not $controller.status.present -or $null -eq $controller.status.problem_code -or
            [uint32]$controller.status.problem_code -ne 0 -or
            $controller.service -notin @('BTHUSB', 'IBTUSB') -or
            [string]::IsNullOrWhiteSpace([string]$controller.driver.inf_path) -or
            [string]::IsNullOrWhiteSpace([string]$controller.driver.provider) -or
            $controller.driver.provider -eq 'DirectHCI Project') {
            throw "Windows Bluetooth ownership is not confirmed: $($controller.identity.instance_id)"
        }
        $security = Get-DeviceSecurityOverride -InstanceId $controller.identity.instance_id
        if ($security -ceq 'D:P(A;;GA;;;SY)(A;;GA;;;BA)') {
            if ($RepairLegacySecurity) {
                $repair = Join-Path $PSScriptRoot 'repair-bluetooth-security.ps1'
                if (-not (Test-Path -LiteralPath $repair -PathType Leaf)) { throw "Missing repair helper: $repair" }
                & $repair -InstanceId $controller.identity.instance_id -Apply
                throw 'Device-security repair requested. Choose Windows Restart and verify Windows Bluetooth before rerunning cleanup. Packages, certificates and state have not been deleted.'
            }
            throw "Legacy DirectHCI device-security override remains on $($controller.identity.instance_id). Use repair-bluetooth-security.ps1 for this exact device, then Restart and verify Windows Bluetooth before retrying."
        }
    }
}

function Assert-Idle {
    $service = @(Get-Service | Where-Object { $_.Name -eq 'DirectHCI' })
    if ($service.Count -gt 0 -and ($FullUninstall -or $service[0].Status -ne 'Stopped')) {
        throw 'DirectHCI service has not reached the required stopped/removed state; cleanup stopped.'
    }
    if (@(Get-Process -Name directhcid -ErrorAction SilentlyContinue).Count -gt 0) {
        throw 'A directhcid process remains; it will not be killed.'
    }
    if (Test-Path -LiteralPath $journal) { throw 'Unresolved journal retained; refusing state deletion.' }
    $task = @(Get-ScheduledTask | Where-Object { $_.TaskName -eq 'DirectHCI Boot Recovery' -and $_.TaskPath -eq '\' })
    if ($task.Count -gt 0 -and ($FullUninstall -or $task[0].State -eq 'Running')) {
        throw 'Boot recovery task has not reached the required idle/removed state; keep binaries and recovery state.'
    }
}

function Invoke-Native([string] $Program, [string[]] $Arguments) {
    & $Program @Arguments
    if ($LASTEXITCODE -eq 3010) { throw 'Windows requires Restart. Stop here, Restart, then rerun cleanup.' }
    if ($LASTEXITCODE -ne 0) { throw "Command failed (exit $LASTEXITCODE): $Program $($Arguments -join ' ')" }
}

function Assert-TrustedRecoveryAcl([string] $Path, [long] $ForbiddenRights, [bool] $RequireProtected) {
    $acl = Get-Acl -LiteralPath $Path
    $descriptor = [Security.AccessControl.RawSecurityDescriptor]::new($acl.Sddl)
    $trusted = @('S-1-5-18', 'S-1-5-32-544',
        'S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464')
    if ($null -eq $descriptor.Owner -or $descriptor.Owner.Value -notin $trusted -or
        $null -eq $descriptor.DiscretionaryAcl -or
        ($RequireProtected -and -not $acl.AreAccessRulesProtected)) {
        throw "Recovery input has an untrusted owner, missing DACL or inherited directory DACL: $Path"
    }
    foreach ($ace in $descriptor.DiscretionaryAcl) {
        if ($ace -isnot [Security.AccessControl.CommonAce] -or $ace.IsCallback -or
            $ace.AceQualifier -notin @([Security.AccessControl.AceQualifier]::AccessAllowed,
                [Security.AccessControl.AceQualifier]::AccessDenied)) {
            throw "Unfamiliar recovery-input ACE; journal retained: $Path"
        }
        # Windows PowerShell can throw InvalidCastException when -band binds
        # byte-backed AceFlags directly. Compare integer masks instead; the
        # inherit-only exemption and all ACL rejection rules remain unchanged.
        $aceFlags = [int]$ace.AceFlags
        $inheritOnly = [int][Security.AccessControl.AceFlags]::InheritOnly
        if (($aceFlags -band $inheritOnly) -ne 0) { continue }
        if ($ace.AceQualifier -eq [Security.AccessControl.AceQualifier]::AccessAllowed -and
            $ace.SecurityIdentifier.Value -notin $trusted -and
            ([long]$ace.AccessMask -band $ForbiddenRights) -ne 0) {
            throw "Untrusted SID can modify recovery input: $Path"
        }
    }
}

function Assert-ReconciliationIdle {
    $service = @(Get-Service | Where-Object { $_.Name -eq 'DirectHCI' })
    if (($service.Count -gt 0 -and $service[0].Status -ne 'Stopped') -or
        @(Get-Process -Name @('directhcid', 'directhci') -ErrorAction SilentlyContinue).Count -gt 0) {
        throw 'Reconciliation requires a stopped service and no directhcid/directhci process; nothing will be killed.'
    }
    $runningTasks = @(Get-ScheduledTask | Where-Object {
        $_.TaskName -eq 'DirectHCI Boot Recovery' -and $_.State -eq 'Running'
    })
    if ($runningTasks.Count -gt 0) { throw 'Boot recovery is running; journal retained.' }
}

function Get-ReconciledWindowsController($Record) {
    Assert-ReconciliationIdle
    $controllers = @(Get-ControllerState)
    Assert-WindowsOwned $controllers
    $matches = @($controllers | Where-Object {
        $_.identity.instance_id -ieq $Record.controller_identity.instance_id
    })
    if ($matches.Count -ne 1) { throw 'Journal controller is missing or ambiguous; journal retained.' }
    $controller = $matches[0]
    $historical = $Record.pre_acquire_observation.driver
    if ($controller.identity.usb_vendor_id -ne $Record.controller_identity.usb_vendor_id -or
        $controller.identity.usb_product_id -ne $Record.controller_identity.usb_product_id -or
        [guid]$controller.class_guid -ne [guid]'e0cbf06c-cd8b-4647-bb8a-263b43f0f974' -or
        [string]::IsNullOrWhiteSpace([string]$historical.inf_path) -or
        [string]::IsNullOrWhiteSpace([string]$historical.provider) -or
        [string]::IsNullOrWhiteSpace([string]$historical.version) -or
        [IO.Path]::GetFileName($controller.driver.inf_path) -ine [IO.Path]::GetFileName($historical.inf_path) -or
        $controller.driver.provider -ine $historical.provider -or $controller.driver.version -ine $historical.version) {
        throw 'Controller identity or original Windows driver differs; no automatic journal reconciliation.'
    }
    if ($null -eq $controller.status.status_flags -or
        ([uint32]$controller.status.status_flags -band 0x8) -eq 0 -or
        (-not $ForceUninstall -and ([uint32]$controller.status.status_flags -band 0x100) -ne 0)) {
        throw 'Controller is not started, status is unavailable, or normal reconciliation still requires DN_NEED_RESTART to clear; journal retained. No driver was installed.'
    }
    if ($ForceUninstall -and ([uint32]$controller.status.status_flags -band 0x100) -ne 0) {
        Write-Warning 'DN_NEED_RESTART is still set. Forced uninstall will remove DirectHCI without claiming that pending Windows recovery or radio functionality is complete.'
    }
    if ([DirectHciCleanupInterfaceV1]::HasActiveInterface($controller.identity.instance_id, [guid]$guid)) {
        throw 'DirectHCI application interface is still active; journal retained.'
    }
    return $controller
}

function Reconcile-CompletedWindowsRecovery {
    if (-not (Test-Path -LiteralPath $journal)) { return }
    Assert-ReconciliationIdle
    Assert-PlainTree $stateDir
    # Include both the input file's independent DACL and its directory/parent.
    Assert-TrustedRecoveryAcl ([IO.Path]::GetDirectoryName($stateDir)) 0x100D0040 $false
    Assert-TrustedRecoveryAcl $stateDir 0x500D0156 $true
    Assert-TrustedRecoveryAcl $journal 0x500D0156 $false
    $file = Get-Item -LiteralPath $journal -Force
    if ($file.PSIsContainer -or $file.Length -gt 1048576) { throw 'Unexpected journal file; retained.' }
    $hash = (Get-FileHash -LiteralPath $journal -Algorithm SHA256).Hash
    $record = Get-Content -LiteralPath $journal -Raw -Encoding UTF8 | ConvertFrom-Json
    if ($record.schema_version -ne 1 -or $record.backend -cne 'temporary_win_usb_rebind' -or
        $record.phase -cne 'recovery_required' -or $record.recovery_desired_state -cne 'windows_owned' -or
        $record.controller_identity.instance_id -notmatch '^USB\\VID_[0-9A-F]{4}&PID_[0-9A-F]{4}[^\\]*\\[^\\]+$' -or
        [uint64]$record.created_unix_ms -eq 0 -or [uint64]$record.updated_unix_ms -lt [uint64]$record.created_unix_ms -or
        [guid]$record.directhci_driver_package.device_interface_guid -ne [guid]$guid) {
        throw 'Journal is not a recognized completed-recovery candidate; retained.'
    }
    $bootUtc = (Get-CimInstance -ClassName Win32_OperatingSystem).LastBootUpTime.ToUniversalTime()
    $bootMs = [DateTimeOffset]::new($bootUtc).ToUnixTimeMilliseconds()
    Write-Host ('Last boot UTC: {0:o}; last recovery journal update UTC: {1:o}' -f $bootUtc,
        [DateTimeOffset]::FromUnixTimeMilliseconds([long]$record.updated_unix_ms).UtcDateTime)
    # A restart before the LAST restore attempt does not complete that attempt.
    # Never reinstall the driver just to reconcile an already-restored binding.
    if (-not $ForceUninstall -and $bootMs -le ([long]$record.updated_unix_ms + 2000)) {
        throw 'The last recovery attempt happened after the last restart. No driver was installed. Choose Windows Restart, then rerun this SAME cleanup command with -ReconcileWindowsOwned; do NOT run the old recover/uninstall command or start consumers.'
    }
    if (-not ('DirectHciCleanupInterfaceV1' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class DirectHciCleanupInterfaceV1 {
    [DllImport("cfgmgr32.dll", CharSet = CharSet.Unicode, ExactSpelling = true)]
    private static extern uint CM_Get_Device_Interface_List_SizeW(out uint size, ref Guid guid, string id, uint flags);
    [DllImport("cfgmgr32.dll", CharSet = CharSet.Unicode, ExactSpelling = true)]
    private static extern uint CM_Get_Device_Interface_ListW(ref Guid guid, string id, [Out] char[] buffer, uint size, uint flags);
    public static bool HasActiveInterface(string id, Guid guid) {
        // CM_GET_DEVICE_INTERFACE_LIST_PRESENT = 0; CR_BUFFER_SMALL = 0x1A.
        for (int attempt = 0; attempt < 3; attempt++) {
            uint size;
            uint result = CM_Get_Device_Interface_List_SizeW(out size, ref guid, id, 0);
            if (result != 0) throw new InvalidOperationException("Interface-list size failed: CONFIGRET 0x" + result.ToString("X"));
            if (size < 1 || size > 65536) throw new InvalidOperationException("Unexpected interface-list size");
            char[] buffer = new char[(int)size];
            result = CM_Get_Device_Interface_ListW(ref guid, id, buffer, size, 0);
            if (result == 0x1A) continue;
            if (result != 0) throw new InvalidOperationException("Interface-list query failed: CONFIGRET 0x" + result.ToString("X"));
            return buffer[0] != '\0';
        }
        throw new InvalidOperationException("Interface list kept changing; journal retained");
    }
}
'@
    }
    $controller = Get-ReconciledWindowsController $record
    [pscustomobject]@{
        Purpose = $(if ($ForceUninstall) { 'Explicit forced uninstall; current Windows binding checked, reboot/radio recovery NOT certified' }
            else { 'Completed Windows restore reconciliation, not a PnP transition' })
        ForcedUninstall = [bool]$ForceUninstall
        BootUtc = $bootUtc; JournalSHA256 = $hash; Controller = $controller
    } | ConvertTo-Json -Depth 12 |
        Set-Content -LiteralPath (Join-Path $backup 'recovery-reconciliation.json') -Encoding UTF8
    # Re-observe immediately before moving the verified stale record. Keep the
    # original bytes, not an edited phase/time/owner that could defeat old guards.
    $null = Get-ReconciledWindowsController $record
    Assert-ReconciliationIdle
    Assert-PlainTree $stateDir
    Assert-TrustedRecoveryAcl $journal 0x500D0156 $false
    if ((Get-FileHash -LiteralPath $journal -Algorithm SHA256).Hash -cne $hash) {
        throw 'Journal changed during reconciliation; retained.'
    }
    $archiveName = $(if ($ForceUninstall) { 'forced-uninstall-ownership-journal-v1.json' }
        else { 'reconciled-ownership-journal-v1.json' })
    Move-Item -LiteralPath $journal -Destination (Join-Path $backup $archiveName)
    if ($ForceUninstall) {
        Write-Warning 'Original journal archived under explicit forced-uninstall authorization. Current Windows binding was verified, not completed reboot recovery. No driver was installed.'
    } else {
        Write-Host 'Fresh PnP/driver/interface checks confirm completed restore; original journal archived without installing a driver.'
    }
}

function Stop-RecoveryForForcedUninstall {
    # Validate attribution before disabling anything. Prevent a boot task or
    # service restart from issuing another restore while its journal is archived.
    $registration = @(Get-CimInstance -ClassName Win32_Service -Filter "Name='DirectHCI'")
    if ($registration.Count -gt 1) { throw 'Ambiguous DirectHCI service registration.' }
    if ($registration.Count -eq 1) {
        $image = [Environment]::ExpandEnvironmentVariables([string]$registration[0].PathName).Trim()
        $expected = '"' + $daemon + '" service'
        if (-not $image.Equals($expected, [StringComparison]::OrdinalIgnoreCase)) {
            throw "Service targets an unfamiliar executable; registration retained: $image"
        }
    }
    $tasks = @(Get-ScheduledTask | Where-Object {
        $_.TaskName -eq 'DirectHCI Boot Recovery' -and $_.TaskPath -eq '\'
    })
    foreach ($task in $tasks) {
        $actions = @($task.Actions)
        if ($actions.Count -ne 1 -or [string]::IsNullOrWhiteSpace($actions[0].Execute)) {
            throw 'Unfamiliar boot recovery action; forced removal stopped.'
        }
        $executable = [Environment]::ExpandEnvironmentVariables([string]$actions[0].Execute).Trim('"')
        if (-not [IO.Path]::IsPathRooted($executable) -or
            -not ([IO.Path]::GetFullPath($executable)).Equals($daemon, [StringComparison]::OrdinalIgnoreCase)) {
            throw "Boot recovery targets another installation; task retained: $executable"
        }
    }
    foreach ($task in $tasks) {
        Export-ScheduledTask -TaskName $task.TaskName -TaskPath $task.TaskPath |
            Set-Content -LiteralPath (Join-Path $backup 'boot-recovery-task-before-disable.xml') -Encoding Unicode
        Disable-ScheduledTask -TaskName $task.TaskName -TaskPath $task.TaskPath | Out-Null
    }
    if ($registration.Count -eq 1) {
        $registration | Select-Object -Property @('Name', 'State', 'StartMode', 'PathName') |
            ConvertTo-Json | Set-Content -LiteralPath (Join-Path $backup 'service-before-disable.json') -Encoding UTF8
        Set-Service -Name DirectHCI -StartupType Disabled
        $live = Get-Service -Name DirectHCI
        try {
            if ($live.Status -ne [ServiceProcess.ServiceControllerStatus]::Stopped) {
                Stop-Service -Name DirectHCI
                $live.WaitForStatus([ServiceProcess.ServiceControllerStatus]::Stopped, [TimeSpan]::FromSeconds(90))
            }
        } finally { $live.Dispose() }
    }
    Assert-ReconciliationIdle
}

function Remove-ForcedServiceRegistration {
    # Use SCM, not registry deletion or the old offline-recovery executable.
    # Only a stopped, attributed registration may be removed by this mode.
    Assert-ReconciliationIdle
    if (Test-Path -LiteralPath $journal) { throw 'Journal has not been safely archived; service retained.' }
    Assert-WindowsOwned @(Get-ControllerState)
    $registration = @(Get-CimInstance -ClassName Win32_Service -Filter "Name='DirectHCI'")
    if ($registration.Count -eq 0) { return }
    $image = [Environment]::ExpandEnvironmentVariables([string]$registration[0].PathName).Trim()
    if ($registration.Count -ne 1 -or $registration[0].State -ne 'Stopped' -or
        -not $image.Equals(('"' + $daemon + '" service'), [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Service identity/state changed; forced deletion refused.'
    }
    $sc = Join-Path ([Environment]::GetFolderPath('System')) 'sc.exe'
    & $sc delete DirectHCI
    if ($LASTEXITCODE -notin @(0, 1072)) { throw "SCM service deletion failed: $LASTEXITCODE" }
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while (Test-Path -LiteralPath $serviceKey) {
        if ([DateTime]::UtcNow -ge $deadline) {
            throw 'SCM deletion is still pending. Close Services/Computer Management and other service-monitor windows, then rerun this same forced cleanup. Recovery is disabled; do not delete the registry service key manually.'
        }
        Start-Sleep -Milliseconds 250
    }
    Write-Host 'DirectHCI service registration removed through SCM without invoking offline recovery.'
}

function Remove-LeftoverBootRecoveryTask {
    # Older installed runtimes do not know about tasks created by newer builds.
    # Remove only this exact task with an action targeting this installation.
    $tasks = @(Get-ScheduledTask | Where-Object {
        $_.TaskName -eq 'DirectHCI Boot Recovery' -and $_.TaskPath -eq '\'
    })
    foreach ($task in $tasks) {
        if ($task.State -eq 'Running' -or
            @(Get-Process -Name directhcid -ErrorAction SilentlyContinue).Count -gt 0 -or
            (Test-Path -LiteralPath $journal)) {
            throw 'Boot recovery is running or unresolved; task and recovery files retained.'
        }
        $actions = @($task.Actions)
        if ($actions.Count -ne 1 -or [string]::IsNullOrWhiteSpace($actions[0].Execute)) {
            throw 'Unfamiliar boot-recovery task action; task retained for review.'
        }
        $executable = [Environment]::ExpandEnvironmentVariables([string]$actions[0].Execute).Trim('"')
        if (-not [IO.Path]::IsPathRooted($executable) -or
            -not ([IO.Path]::GetFullPath($executable)).Equals($daemon, [StringComparison]::OrdinalIgnoreCase)) {
            throw "Boot-recovery task targets another installation; task retained: $executable"
        }
        Export-ScheduledTask -TaskName $task.TaskName -TaskPath $task.TaskPath |
            Set-Content -LiteralPath (Join-Path $backup 'boot-recovery-task.xml') -Encoding Unicode
        Unregister-ScheduledTask -TaskName $task.TaskName -TaskPath $task.TaskPath -Confirm:$false
        Write-Host 'Removed leftover DirectHCI Boot Recovery task through Task Scheduler.'
    }
}

function Move-InstalledResidue([string] $Path, [string] $Name) {
    Assert-PlainTree $Path
    $destination = Join-Path $backup $Name
    for ($attempt = 0; $attempt -lt 10; $attempt++) {
        try {
            Move-Item -LiteralPath $Path -Destination $destination -ErrorAction Stop
            return
        } catch [IO.IOException] {
            if ($attempt -eq 9 -or (Test-Path -LiteralPath $destination)) {
                throw "Residue retained at $Path (or $destination): $($_.Exception.Message). Close Explorer and other terminals using this directory, then rerun. No process will be killed and no directory will be force-deleted."
            }
            Start-Sleep -Milliseconds 500
        }
    }
}

function Get-DevelopmentArtifacts {
    # Only known DirectHCI output directories, never a user/profile/workspace
    # root or an arbitrary CARGO_TARGET_DIR. Also inspect other local profiles
    # because elevated builds may have run under a different Windows account.
    $roots = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    [void]$roots.Add((Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'DirectHCI'))
    foreach ($profile in Get-CimInstance -ClassName Win32_UserProfile) {
        if ($profile.Special -or [string]::IsNullOrWhiteSpace($profile.LocalPath)) { continue }
        $root = Join-Path $profile.LocalPath 'AppData\Local\DirectHCI'
        if (Test-Path -LiteralPath $root -PathType Container) { [void]$roots.Add($root) }
    }
    foreach ($root in $roots) {
        foreach ($name in @('target', 'native', 'libwdi-build', 'driver', 'installer-output', 'recovery-backups')) {
            $path = Join-Path $root $name
            if (Test-Path -LiteralPath $path) {
                Assert-PlainTree $path
                [pscustomobject]@{ Path = [IO.Path]::GetFullPath($path); Kind = $name }
            }
        }
    }
}

function Assert-NoOtherCertificateUsesDevelopmentKey($Entries) {
    $keys = @($Entries | Where-Object { $_.Certificate.HasPrivateKey })
    if ($keys.Count -eq 0) { return }
    $selected = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($entry in $Entries) { [void]$selected.Add($entry.Certificate.Thumbprint) }
    $all = @(Get-ChildItem -Path 'Cert:\CurrentUser', 'Cert:\LocalMachine' -Recurse |
        Where-Object { $_ -is [Security.Cryptography.X509Certificates.X509Certificate2] })
    foreach ($entry in $keys) {
        foreach ($other in $all) {
            if ($other.GetPublicKeyString() -ceq $entry.Certificate.GetPublicKeyString() -and
                -not $selected.Contains($other.Thumbprint)) {
                throw "Development key is shared with an unselected certificate: $($other.Subject) / $($other.Thumbprint); key retained."
            }
        }
    }
}

function Get-DevelopmentKeyIdentity($Certificate) {
    # Copies in Root/TrustedPublisher/My may point at the same key; copies in
    # LocalMachine and CurrentUser can instead point at DIFFERENT containers.
    # Track the actual provider/container, not just the certificate thumbprint.
    $rsa = [Security.Cryptography.X509Certificates.RSACertificateExtensions]::GetRSAPrivateKey($Certificate)
    if ($null -eq $rsa) { throw 'Unknown development signing-key algorithm; retain for review.' }
    try {
        if ($rsa -is [Security.Cryptography.RSACng]) {
            return ('CNG:{0}:{1}:{2}' -f $rsa.Key.IsMachineKey, $rsa.Key.Provider.Provider, $rsa.Key.UniqueName)
        }
        if ($rsa -is [Security.Cryptography.RSACryptoServiceProvider]) {
            $info = $rsa.CspKeyContainerInfo
            return ('CSP:{0}:{1}:{2}:{3}' -f $info.MachineKeyStore, $info.ProviderName, $info.ProviderType, $info.UniqueKeyContainerName)
        }
        throw 'Unknown development signing-key provider; retain for review.'
    } finally { $rsa.Dispose() }
}

function Get-CertificateIfPresent([string] $Path) {
    # Only absence is idempotent success. Access/provider errors still stop
    # cleanup; do not hide them with -ErrorAction SilentlyContinue.
    try { return Get-Item -Path $Path -ErrorAction Stop }
    catch [System.Management.Automation.ItemNotFoundException] { return $null }
}

Assert-PlainTree $installDir
Assert-PlainTree $stateDir
$drivers = Get-DriverInventory
$owned = @(Get-OwnedDrivers $drivers)
$stores = @('Cert:\LocalMachine\Root', 'Cert:\LocalMachine\TrustedPublisher',
    'Cert:\CurrentUser\Root', 'Cert:\CurrentUser\TrustedPublisher')
if ($RemoveDevelopmentSigningKeys) {
    $stores += @('Cert:\LocalMachine\My', 'Cert:\CurrentUser\My')
}
$artifacts = @()
if ($RemoveDevelopmentArtifacts) { $artifacts = @(Get-DevelopmentArtifacts) }
$subjects = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
$thumbprints = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)

# Preserve the ownership evidence tying one-time certificates to this installation.
if (Test-Path -LiteralPath $stateDir) {
    foreach ($file in Get-ChildItem -LiteralPath $stateDir -Recurse -File -Filter metadata.json) {
        $metadata = Get-Content -LiteralPath $file.FullName -Raw | ConvertFrom-Json
        $property = $metadata.PSObject.Properties['certificate_subject']
        if ($null -ne $property -and $property.Value -match '^CN=DirectHCI-[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$') {
            [void]$subjects.Add([string]$property.Value)
        }
    }
}
foreach ($driver in $owned) {
    foreach ($cat in Get-ChildItem -LiteralPath ([IO.Path]::GetDirectoryName($driver.OriginalFileName)) -Filter '*.cat' -File) {
        $signature = Get-AuthenticodeSignature -LiteralPath $cat.FullName
        if ($null -eq $signature.SignerCertificate) { throw "Cannot identify catalog signer: $($cat.FullName)" }
        [void]$thumbprints.Add($signature.SignerCertificate.Thumbprint)
    }
}
$certificates = @(foreach ($store in $stores) {
    foreach ($cert in Get-ChildItem -LiteralPath $store) {
        $known = ($FullUninstall -and $subjects.Contains($cert.Subject)) -or $thumbprints.Contains($cert.Thumbprint) -or
            $cert.Subject -ceq 'CN=DirectHCI Development Test'
        if ($known) {
            $allowedDevelopmentKey = $RemoveDevelopmentSigningKeys -and $cert.Subject -ceq 'CN=DirectHCI Development Test'
            if ($cert.Subject -cne $cert.Issuer -or ($cert.HasPrivateKey -and -not $allowedDevelopmentKey)) {
                throw "Unexpected CA/private-key certificate; retain for manual review: $store\$($cert.Thumbprint)"
            }
            $keyIdentity = $null
            if ($cert.HasPrivateKey) { $keyIdentity = Get-DevelopmentKeyIdentity $cert }
            [pscustomobject]@{ Store = $store; Certificate = $cert; KeyIdentity = $keyIdentity }
        }
    }
})
$unidentified = @(foreach ($store in $stores) {
    foreach ($cert in Get-ChildItem -LiteralPath $store) {
        if ($cert.Subject -match '^CN=DirectHCI-' -and -not $subjects.Contains($cert.Subject) -and
            -not $thumbprints.Contains($cert.Thumbprint)) {
            [pscustomobject]@{ Store = $store; Subject = $cert.Subject; Thumbprint = $cert.Thumbprint }
        }
    }
})

Write-Host 'DirectHCI cleanup targets (Microsoft/Intel/Broadcom/Realtek driver packages are NOT targets):'
Get-CimInstance -ClassName Win32_Service -Filter "Name='DirectHCI'" |
    Select-Object Name, State, StartMode, PathName | Format-List
Get-ScheduledTask | Where-Object { $_.TaskName -eq 'DirectHCI Boot Recovery' } |
    Select-Object TaskName, TaskPath, State, Actions | Format-List
if ($FullUninstall) {
    foreach ($key in @($serviceKey, $uninstallKey, $eventSourceKey)) {
        if (Test-Path -LiteralPath $key) { Write-Host "System registration: $key" }
    }
}
if (Test-Path -LiteralPath $stateDir) {
    Get-Acl -LiteralPath $stateDir | Format-List Owner, AreAccessRulesProtected, Sddl
}
$owned | Format-Table Driver, ProviderName, OriginalFileName -AutoSize
$certificates | Select-Object Store, @{n='Subject';e={$_.Certificate.Subject}}, @{n='Thumbprint';e={$_.Certificate.Thumbprint}} | Format-Table -AutoSize
if ($unidentified.Count -gt 0) {
    Write-Warning 'DirectHCI-named certificates without package/metadata attribution require review:'
    $unidentified | Format-Table -AutoSize
}
if ($FullUninstall) {
    Write-Host "FULL uninstall: installed files $installDir and runtime state $stateDir are targets."
    if ($ReconcileWindowsOwned) {
        Write-Host 'Completed-recovery reconciliation requested: requires a restart after the last journal update, stopped runtime, exact original Windows driver, started controller, no DN_NEED_RESTART and no DirectHCI interface. Original journal will be archived only after fresh verification.'
    }
    if ($ForceUninstall) {
        Write-Warning 'FORCED uninstall: disables DirectHCI service/boot recovery, verifies current Windows driver binding, archives the original journal and removes the installation WITHOUT waiting for a restart or DN_NEED_RESTART to clear. It does not certify Windows radio functionality. Used/foreign drivers are never force-deleted.'
    }
} else {
    Write-Host 'LEGACY cleanup: keep the current application, service registration, boot task, device-specific packages and preferences.'
}
if ($RemoveDevelopmentArtifacts) {
    Write-Host 'Development output directories to DELETE (source repository and EWDK/toolchains are NOT targets):'
    $artifacts | Format-Table Path, Kind -AutoSize
    if ($env:CARGO_TARGET_DIR) {
        $customTarget = [IO.Path]::GetFullPath($env:CARGO_TARGET_DIR)
        if ($customTarget -notin @($artifacts | Select-Object -ExpandProperty Path)) {
            Write-Warning "CARGO_TARGET_DIR is not an inventoried existing output directory and will not be deleted: $customTarget"
        }
    }
} else {
    Write-Host 'Development target/native/driver/installer build files will NOT be removed.'
}
if ($RemoveDevelopmentSigningKeys) {
    Write-Warning 'Identified DirectHCI Development Test private keys will be permanently destroyed after package-reference checks. They are NOT backed up as PFX.'
    Assert-NoOtherCertificateUsesDevelopmentKey $certificates
} else {
    Write-Host 'Development private signing keys will NOT be removed.'
}
Get-ChildItem -Path 'Cert:\CurrentUser\My', 'Cert:\LocalMachine\My' |
    Where-Object { $_.Subject -ceq 'CN=DirectHCI Development Test' } |
    Select-Object Subject, Thumbprint, HasPrivateKey | Format-Table -AutoSize
Write-Warning 'Certificate cleanup covers LocalMachine and the current Windows account only. Certificates/private keys belonging to other accounts require that account to run its own inventory; their profile key directories are never erased.'
if (-not $Apply) {
    Write-Host 'Preview only. Close consumers and the Control Panel, then use -Apply.'
    return
}
if ($FullUninstall -and $unidentified.Count -gt 0) {
    throw 'Unattributed certificates found. Save the preview output for review; no cleanup performed.'
}
if (@(Get-Process -Name directhci-control-panel, directhci-ble -ErrorAction SilentlyContinue).Count -gt 0) {
    throw 'Close the Control Panel and BLE CLI first. Close all other DirectHCI consumers too.'
}
$confirmation = $(if ($ForceUninstall) { 'FORCE-UNINSTALL' } else { 'CLEAN' })
if ((Read-Host "Selected cleanup removes listed installation/packages/trust. Type $confirmation to continue") -cne $confirmation) {
    throw 'Cancelled; no changes made.'
}

$backupRoot = Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'DirectHCI-cleanup-backups'
Assert-PlainTree $backupRoot
$backup = Join-Path $backupRoot ([DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss') + '-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $backup -Force | Out-Null
Write-Host "Backup/report directory: $backup"
# Neither the PowerShell location nor the OS working directory may hold the
# installation directory open while Inno removes it. Other processes are not killed.
Set-Location -LiteralPath $backup
[Environment]::CurrentDirectory = $backup
$drivers | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $backup 'drivers-before.json') -Encoding UTF8
if (Test-Path -LiteralPath $stateDir) {
    Copy-Item -LiteralPath $stateDir -Destination (Join-Path $backup 'state-before') -Recurse
}
$before = @(Get-ControllerState)
$before | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath (Join-Path $backup 'controllers-before.json') -Encoding UTF8

# Normal cleanup uses the official recovery path. Explicit forced uninstall
# instead freezes recovery, verifies current binding and archives the journal;
# it never kills a process, installs a driver or edits the service registry key.
if ($FullUninstall) {
    if ($ForceUninstall) {
        Stop-RecoveryForForcedUninstall
        Reconcile-CompletedWindowsRecovery
        Remove-ForcedServiceRegistration
    } else {
        if ($ReconcileWindowsOwned) { Reconcile-CompletedWindowsRecovery }
        if (Test-Path -LiteralPath $daemon -PathType Leaf) { Invoke-Native $daemon @('uninstall-service') }
    }
    $service = @(Get-Service | Where-Object { $_.Name -eq 'DirectHCI' })
    if ($service.Count -gt 0) { throw 'Service removal was not confirmed; registrations and files retained.' }
    Assert-WindowsOwned @(Get-ControllerState)
    Remove-LeftoverBootRecoveryTask
} else {
    $service = @(Get-Service | Where-Object { $_.Name -eq 'DirectHCI' })
    if ($service.Count -gt 0 -and $service[0].Status -ne 'Stopped') {
        Stop-Service -Name DirectHCI
        $service[0].WaitForStatus([ServiceProcess.ServiceControllerStatus]::Stopped, [TimeSpan]::FromSeconds(90))
    }
    if (@(Get-Process -Name directhcid -ErrorAction SilentlyContinue).Count -gt 0) {
        throw 'A daemon remains after service stop; it will not be killed.'
    }
    if (Test-Path -LiteralPath $cli -PathType Leaf) { Invoke-Native $cli @('recover', '--offline', '--json') }
}
Assert-Idle
Assert-WindowsOwned @(Get-ControllerState)
Write-Warning 'Windows driver/PnP ownership has been checked, NOT Windows radio functionality. Cleanup does not prove that Bluetooth can be turned on and will not reset/rebind the radio.'
if ((Read-Host 'Remove ONLY unused DirectHCI packages and their unreferenced trust. Type REMOVE-UNUSED to continue') -cne 'REMOVE-UNUSED') {
    throw "Unused-package cleanup not authorized; packages/state retained. Backup: $backup"
}

# Export each positively identified package, then ask Windows to delete only an
# unused package. NEVER use /force, /uninstall, /install, or delete DriverStore files.
foreach ($driver in $owned) {
    Assert-Idle
    $destination = Join-Path $backup $driver.Driver
    New-Item -ItemType Directory -Path $destination | Out-Null
    Invoke-Native $pnputil @('/export-driver', $driver.Driver, $destination)
    Invoke-Native $pnputil @('/delete-driver', $driver.Driver)
}
$remaining = Get-DriverInventory
if (@(Get-OwnedDrivers $remaining).Count -gt 0) { throw 'A selected DirectHCI package remains; certificates/state retained.' }
Assert-WindowsOwned @(Get-ControllerState)

# Check references to the selected DirectHCI certificates, not the current
# trust-policy health of every unrelated third-party package on this machine.
# Build(false) is NOT synonymous with a missing issuer/root: an expired,
# untrusted or policy-rejected but fully resolved chain still exposes its roots.
if ($certificates.Count -gt 0) {
    Add-Type -AssemblyName System.Security
    $references = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    $chainDiagnostics = [Collections.Generic.List[object]]::new()
    foreach ($driver in $remaining) {
        $directory = [IO.Path]::GetDirectoryName($driver.OriginalFileName)
        $cats = @(Get-ChildItem -LiteralPath $directory -Filter '*.cat' -File)
        if ($cats.Count -eq 0) { throw "No catalog found for remaining package $($driver.Driver); trust retained." }
        foreach ($cat in $cats) {
            $signature = Get-AuthenticodeSignature -LiteralPath $cat.FullName
            if ($null -eq $signature.SignerCertificate) { throw "Unknown signer: $($cat.FullName); trust retained." }
            # Catalogs embed intermediate certificates which the plain
            # X509Chain.Build(signer) call was not given by the old script.
            # Read all embedded certificates/signers, including multi-signed
            # catalogs; none are installed into a Windows certificate store.
            $cms = [Security.Cryptography.Pkcs.SignedCms]::new()
            try { $cms.Decode([IO.File]::ReadAllBytes($cat.FullName)) }
            catch { throw "Cannot read catalog certificate references: $($cat.FullName): $($_.Exception.Message); trust retained." }
            foreach ($embedded in $cms.Certificates) { [void]$references.Add($embedded.Thumbprint) }
            $signers = @($signature.SignerCertificate)
            foreach ($signerInfo in $cms.SignerInfos) {
                if ($null -eq $signerInfo.Certificate) { throw "Catalog signer certificate unavailable: $($cat.FullName); trust retained." }
                $signers += $signerInfo.Certificate
            }
            foreach ($signer in @($signers | Sort-Object Thumbprint -Unique)) {
                $chain = [Security.Cryptography.X509Certificates.X509Chain]::new()
                try {
                    $chain.ChainPolicy.RevocationMode = [Security.Cryptography.X509Certificates.X509RevocationMode]::NoCheck
                    $chain.ChainPolicy.VerificationFlags = [Security.Cryptography.X509Certificates.X509VerificationFlags]::IgnoreNotTimeValid
                    $chain.ChainPolicy.ExtraStore.AddRange($cms.Certificates)
                    foreach ($entry in $certificates) { [void]$chain.ChainPolicy.ExtraStore.Add($entry.Certificate) }
                    $trusted = $chain.Build($signer)
                    foreach ($element in $chain.ChainElements) { [void]$references.Add($element.Certificate.Thumbprint) }
                    $statuses = @($chain.ChainStatus | ForEach-Object { [string]$_.Status })
                    $chainDiagnostics.Add([pscustomobject]@{
                        Catalog = $cat.FullName; Signer = $signer.Subject
                        PolicyAccepted = $trusted; Status = $statuses
                    })
                    $partial = @($chain.ChainStatus | Where-Object {
                        ($_.Status -band [Security.Cryptography.X509Certificates.X509ChainStatusFlags]::PartialChain) -ne 0
                    }).Count -gt 0
                    if ($chain.ChainElements.Count -eq 0 -or $partial) {
                        $chainDiagnostics | ConvertTo-Json -Depth 5 |
                            Set-Content -LiteralPath (Join-Path $backup 'catalog-reference-check.json') -Encoding UTF8
                        throw "Catalog issuer/root is unresolved after loading embedded certificates: $($cat.FullName); signer=$($signer.Subject); status=$($statuses -join ', '); DirectHCI certificate dependencies cannot be ruled out, trust retained. See catalog-reference-check.json in $backup."
                    }
                    if (-not $trusted) {
                        Write-Warning "Catalog chain resolved with policy status $($statuses -join ', '): $($cat.FullName). Its certificate references were checked; this unrelated driver/trust is not modified."
                    }
                } finally { $chain.Dispose() }
            }
        }
    }
    $chainDiagnostics | ConvertTo-Json -Depth 5 |
        Set-Content -LiteralPath (Join-Path $backup 'catalog-reference-check.json') -Encoding UTF8
    foreach ($entry in $certificates) {
        if ($references.Contains($entry.Certificate.Thumbprint)) {
            throw "Certificate still referenced by another package: $($entry.Certificate.Thumbprint); trust retained."
        }
    }
    Assert-NoOtherCertificateUsesDevelopmentKey $certificates
    # Remove public copies first, then key-bearing copies. A key can be shared
    # across the selected Root/TrustedPublisher/My copies of the same cert.
    $orderedCertificates = @($certificates | Sort-Object { [int]$_.Certificate.HasPrivateKey })
    $deletedKeys = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($entry in $orderedCertificates) {
        $thumbprint = $entry.Certificate.Thumbprint
        $path = $entry.Store + '\' + $thumbprint
        # CurrentUser logical stores can include certificates from LocalMachine.
        # Removing one view may also remove an entry in the original inventory
        # of another view; refresh rather than attempting a second deletion.
        $current = Get-CertificateIfPresent $path
        if ($null -eq $current) {
            if ($null -ne $entry.KeyIdentity -and -not $deletedKeys.Contains($entry.KeyIdentity)) {
                throw "Key-bearing certificate disappeared before key destruction could be confirmed: $path; private-key cleanup requires review."
            }
            Write-Host "Certificate already absent (shared store view or earlier cleanup): $path"
            continue
        }
        if ($current.Thumbprint -cne $thumbprint -or $current.Subject -cne $entry.Certificate.Subject) {
            throw "Certificate identity changed during cleanup: $path"
        }
        $certificateBackup = Join-Path $backup ($thumbprint + '.cer')
        if (-not (Test-Path -LiteralPath $certificateBackup)) {
            Export-Certificate -Cert $current -FilePath $certificateBackup | Out-Null
        }
        $deleteKey = $null -ne $entry.KeyIdentity -and -not $deletedKeys.Contains($entry.KeyIdentity)
        try {
            if ($deleteKey) {
                Remove-Item -Path $path -DeleteKey -ErrorAction Stop
                [void]$deletedKeys.Add($entry.KeyIdentity)
                Write-Host "Removed development signing certificate AND private key: $path"
            } else {
                Remove-Item -Path $path -ErrorAction Stop
                Write-Host "Removed certificate: $path"
            }
        } catch [System.Management.Automation.ItemNotFoundException] {
            if ($deleteKey -or $null -ne (Get-CertificateIfPresent $path)) { throw }
            Write-Host "Certificate already absent during removal: $path"
        }
    }
    # Do not equate skipping a vanished duplicate with complete cleanup. Read
    # the live logical stores again, including My, before uninstalling files.
    $selectedThumbprints = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($entry in $certificates) { [void]$selectedThumbprints.Add($entry.Certificate.Thumbprint) }
    foreach ($store in $stores) {
        foreach ($certificate in Get-ChildItem -Path $store -ErrorAction Stop) {
            if ($selectedThumbprints.Contains($certificate.Thumbprint)) {
                throw "Selected DirectHCI certificate remains after cleanup: $store\$($certificate.Thumbprint); installation/state retained."
            }
        }
    }
}

# Legacy cleanup stops here: do not uninstall a current device-specific setup.
if (-not $FullUninstall) {
    Write-Host 'Identified legacy development packages/public trust removed. Current runtime registration and preferences retained; service remains stopped.'
    Write-Host "Backups retained: $backup"
    Write-Host 'Choose Windows Restart, verify Windows Bluetooth, then start DirectHCI when needed.'
    return
}

# Inno handles its registration and shortcuts. Do not manually erase uninstall
# registry entries or silently remove files when its recovery guard refuses.
$uninstaller = Join-Path $installDir 'unins000.exe'
if (Test-Path -LiteralPath $uninstaller -PathType Leaf) {
    $log = Join-Path $backup 'uninstall.log'
    $process = Start-Process -FilePath $uninstaller -ArgumentList @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', ('/LOG="' + $log + '"')) -Wait -PassThru
    if ($process.ExitCode -ne 0) { throw "Uninstaller failed (exit $($process.ExitCode)). Inspect $log; keep remaining state." }
} elseif (Test-Path -LiteralPath $uninstallKey) {
    throw 'Registered uninstaller is missing. Restore the installed runtime/uninstaller; do not erase its registration.'
}
if (Test-Path -LiteralPath $uninstallKey) { throw 'Uninstall registration remains; cleanup stopped.' }
Assert-Idle
Assert-WindowsOwned @(Get-ControllerState)
if (Test-Path -LiteralPath $serviceKey) { throw 'DirectHCI service registry key remains; do not erase it manually.' }
if (Test-Path -LiteralPath $eventSourceKey) {
    # Remove the source registration, NOT the Application event log or its history.
    $reg = Join-Path ([Environment]::GetFolderPath('System')) 'reg.exe'
    Invoke-Native $reg @('export', 'HKLM\SYSTEM\CurrentControlSet\Services\EventLog\Application\DirectHCI',
        (Join-Path $backup 'event-source.reg'), '/y')
    Remove-EventLog -Source 'DirectHCI'
    if (Test-Path -LiteralPath $eventSourceKey) { throw 'DirectHCI event-source registration remains.' }
    Write-Host 'Removed DirectHCI event-source registration; Windows event history retained.'
}

# Move exact, inspected residual trees out of their active system locations.
# These backups are not active runtime state and can be removed after acceptance.
foreach ($entry in @(@($stateDir, 'state-residue'), @($installDir, 'installation-residue'))) {
    if (Test-Path -LiteralPath $entry[0]) {
        Move-InstalledResidue $entry[0] $entry[1]
    }
}
if ($RemoveDevelopmentArtifacts) {
    foreach ($artifact in $artifacts) {
        Assert-Idle
        if (-not (Test-Path -LiteralPath $artifact.Path)) { continue }
        Assert-PlainTree $artifact.Path
        Remove-Item -LiteralPath $artifact.Path -Recurse -Force
        Write-Host "Deleted development output (not backed up; rebuild to recreate): $($artifact.Path)"
    }
    $artifactRoots = @($artifacts |
        ForEach-Object { [IO.Path]::GetDirectoryName($_.Path) } | Select-Object -Unique)
    foreach ($root in $artifactRoots) {
        if (-not (Test-Path -LiteralPath $root)) { continue }
        Assert-PlainTree $root
        if (@(Get-ChildItem -LiteralPath $root -Force).Count -eq 0) {
            Remove-Item -LiteralPath $root -Force
        } else {
            Write-Warning "Unclassified files retained for review in: $root"
            Get-ChildItem -LiteralPath $root -Force | Select-Object FullName, Attributes
        }
    }
}
Write-Host 'Active DirectHCI installation, service, boot task, identified driver packages and identified public trust removed.'
Write-Host "Backups retained: $backup"
Write-Host 'Windows event logs, crash dumps, original Bluetooth drivers, source repository, EWDK and unrelated signing keys were NOT deleted.'
Write-Host 'Cleanup is NOT proof that the Windows Bluetooth radio is functional. If it is still unavailable, retain the backup for diagnosis.'
if (-not $ForceUninstall) {
    Write-Host 'Choose Windows Restart before installing the new DirectHCI build. Do not use shutdown/power-on with Fast Startup.'
}
