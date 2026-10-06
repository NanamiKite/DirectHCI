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
  Result := '';
  { First install has no service to replace. Only an existing service or an
    outstanding journal warrants the stop/recover preflight. }
  if not RegKeyExists(HKEY_LOCAL_MACHINE,
    'SYSTEM\CurrentControlSet\Services\DirectHCI') and
    not FileExists(ExpandConstant(
      '{commonappdata}\DirectHCI\ownership-journal-v1.json')) then
    Exit;
  ExtractTemporaryFile('directhcid-preflight.exe');
  Helper := ExpandConstant('{tmp}\directhcid-preflight.exe');
  if not RunDaemon(Helper, 'uninstall-service', ResultCode, Details) then
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
