; DirectHCI Windows x64 installer. No Bluetooth driver is staged by Setup.
; The privileged runtime prepares exact-HWID packages only after GUI consent.
#ifndef DirectHciBinaryDir
  #error Define DirectHciBinaryDir to a local Windows release build directory.
#endif
#ifndef DirectHciLibwdiDir
  #error Define DirectHciLibwdiDir to the pinned locally built libwdi directory.
#endif

[Setup]
AppId={{C6F4C08B-6961-49FE-98AB-6CEA79EC0D6E}
AppName=DirectHCI
AppVersion=0.2.0
AppPublisher=DirectHCI Project
DefaultDirName={autopf}\DirectHCI
DefaultGroupName=DirectHCI
DisableDirPage=yes
PrivilegesRequired=admin
ArchitecturesAllowed=x64os
ArchitecturesInstallIn64BitMode=x64os
OutputBaseFilename=DirectHCI-Setup-0.2.0
Compression=lzma
SolidCompression=yes
CloseApplications=no
RestartApplications=no
SetupIconFile=..\assets\directhci.ico
UninstallDisplayIcon={app}\directhci-control-panel.exe
LicenseFile=..\LICENSE.txt

[Files]
Source: "..\LICENSE.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\assets\directhci.ico"; DestDir: "{app}"; Flags: ignoreversion
; A separate copy permits a safe upgrade check before replacing a running service binary.
Source: "{#DirectHciBinaryDir}\directhcid.exe"; DestName: "directhcid-preflight.exe"; Flags: dontcopy noencryption
Source: "{#DirectHciBinaryDir}\directhcid.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#DirectHciBinaryDir}\directhci.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#DirectHciBinaryDir}\directhci-control-panel.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#DirectHciBinaryDir}\directhci-ble.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#DirectHciLibwdiDir}\libwdi.dll"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\DirectHCI Control Panel"; Filename: "{app}\directhci-control-panel.exe"; IconFilename: "{app}\directhci.ico"
Name: "{group}\Uninstall DirectHCI"; Filename: "{uninstallexe}"

[Code]
function DirectHciRunningError: String;
var
  Locator, Services, Objects, Item: Variant;
  I, Count: Integer;
  ProcessId: Cardinal;
  Details, ProcessName, ServiceState: String;
begin
  Result := '';
  Details := '';
  try
    { Machine-wide queries include tray apps, console runtimes and processes
      in other user sessions. These checks never stop or terminate anything. }
    Locator := CreateOleObject('WbemScripting.SWbemLocator');
    Services := Locator.ConnectServer('.', 'root\CIMV2');
    Objects := Services.ExecQuery(
      'SELECT Name, ProcessId FROM Win32_Process WHERE ' +
      'Name = "directhci-control-panel.exe" OR Name = "directhcid.exe" OR ' +
      'Name = "directhci.exe" OR Name = "directhci-ble.exe" OR ' +
      'Name = "directhcid-preflight.exe"');
    Count := Objects.Count;
    for I := 0 to Count - 1 do begin
      Item := Objects.ItemIndex(I);
      ProcessName := Item.Name;
      ProcessId := Item.ProcessId;
      Details := Details + #13#10 + '  ' + ProcessName +
        ' (PID ' + IntToStr(ProcessId) + ')';
    end;
    Objects := Services.ExecQuery(
      'SELECT State FROM Win32_Service WHERE Name = "DirectHCI"');
    Count := Objects.Count;
    for I := 0 to Count - 1 do begin
      Item := Objects.ItemIndex(I);
      ServiceState := Item.State;
      if CompareText(ServiceState, 'Stopped') <> 0 then
        Details := Details + #13#10 + '  DirectHCI service: ' + ServiceState;
    end;
    if Details <> '' then
      Result := 'DirectHCI is still running. Setup cannot continue.' +
        #13#10#13#10 + 'Disconnect active clients, stop the DirectHCI service, ' +
        'and exit the Control Panel (including the system tray) and CLI tools. ' +
        'Wait for shutdown to finish, then run Setup again.' +
        #13#10#13#10 + 'Detected:' + Details;
  except
    { An unavailable/failed query is not evidence that installation is safe. }
    Result := 'Setup could not verify that DirectHCI is stopped. ' +
      'Installation is blocked; no process or service was stopped.' +
      #13#10#13#10 + GetExceptionMessage;
  end;
end;

function InitializeSetup: Boolean;
var
  Details: String;
begin
  { Abort before displaying the wizard or running the recovery helper. }
  Details := DirectHciRunningError;
  Result := Details = '';
  if not Result then begin
    Log(Details);
    SuppressibleMsgBox(Details, mbError, MB_OK, IDOK);
  end;
end;

function LastNonEmptyLine(const Lines: TArrayOfString): String;
var
  I: Integer;
begin
  Result := '';
  for I := 0 to GetArrayLength(Lines) - 1 do
    if Trim(Lines[I]) <> '' then
      Result := Lines[I];
end;

function RunDaemon(const Exe, Args: String; var ResultCode: Integer;
  var Details: String): Boolean;
var
  Output: TExecOutput;
begin
  Details := '';
  try
    Result := ExecAndCaptureOutput(Exe, Args, '', SW_SHOWNORMAL,
      ewWaitUntilTerminated, ResultCode, Output);
    if Result then begin
      Details := LastNonEmptyLine(Output.StdErr);
      if Details = '' then
        Details := LastNonEmptyLine(Output.StdOut);
      if Output.Error then
        Details := Details + ' (output capture incomplete; inspect Setup log)';
      if Details <> '' then Log(Exe + ' ' + Args + ': ' + Details);
    end else
      Details := SysErrorMessage(ResultCode);
  except
    Result := False;
    Details := GetExceptionMessage;
  end;
  if Length(Details) > 600 then Details := Copy(Details, 1, 600) + '...';
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  ResultCode: Integer;
  Helper: String;
  Details: String;
begin
  { The user may have started DirectHCI while the wizard was open. }
  Result := DirectHciRunningError;
  if Result <> '' then Exit;
  { First install has no service to replace. Only an existing service or an
    outstanding journal warrants the stop/recover preflight. }
  if not RegKeyExists(HKEY_LOCAL_MACHINE,
    'SYSTEM\CurrentControlSet\Services\DirectHCI') and
    not FileExists(ExpandConstant(
      '{commonappdata}\DirectHCI\ownership-journal-v1.json')) then
    Exit;
  ExtractTemporaryFile('directhcid-preflight.exe');
  Helper := ExpandConstant('{tmp}\directhcid-preflight.exe');
  if not RunDaemon(Helper, 'uninstall-service --require-stopped', ResultCode, Details) then
    Result := 'Cannot check existing DirectHCI service: ' + Details
  else if ResultCode <> 0 then
    Result := 'DirectHCI service/recovery preflight failed (exit ' +
      IntToStr(ResultCode) + '). No files were replaced. ' + Details;
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  ResultCode: Integer;
  Exe: String;
  Details: String;
begin
  if CurStep = ssInstall then begin
    { Check again after the helper exits, before replacing any application file. }
    Details := DirectHciRunningError;
    if Details <> '' then RaiseException(Details);
  end;
  if CurStep = ssPostInstall then begin
    Exe := ExpandConstant('{app}\directhcid.exe');
    if not RunDaemon(Exe, 'install-service', ResultCode, Details) then
      RaiseException('Could not register DirectHCI service: ' + Details);
    if ResultCode <> 0 then
      RaiseException('DirectHCI service registration failed (exit ' +
        IntToStr(ResultCode) + '): ' + Details +
        '. Files remain for repair; no driver was changed.');
  end;
end;

function InitializeUninstall: Boolean;
var
  ResultCode: Integer;
  Exe: String;
  Details: String;
begin
  Exe := ExpandConstant('{app}\directhcid.exe');
  Result := FileExists(Exe);
  if not Result then begin
    MsgBox('DirectHCI runtime is missing. Restore the runtime executable before uninstalling so Windows Bluetooth recovery can be verified.', mbError, MB_OK);
    Exit;
  end;
  if not RunDaemon(Exe, 'uninstall-service', ResultCode, Details) then begin
    MsgBox('Could not stop DirectHCI or check Windows Bluetooth recovery: ' +
      Details, mbError, MB_OK);
    Result := False;
  end else if ResultCode <> 0 then begin
    MsgBox('Windows Bluetooth recovery was not confirmed: ' + Details +
      '. Uninstall is cancelled; run directhci recover --offline as Administrator and retry.', mbError, MB_OK);
    Result := False;
  end;
end;
