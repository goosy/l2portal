; installer/setup.iss
; Inno Setup 6 script for L2Portal
; Requires: Inno Setup 6.x  (https://jrsoftware.org/isinfo.php)
;
; Expected layout at compile time (relative to this .iss file's parent = project root):
;   target\release\l2portal.exe
;   installer\l2p.cmd             (short alias — generated here, not from Rust)
;   deps\tap\tapctl.exe
;   deps\tap\amd64\devcon.exe
;   deps\tap\amd64\OemVista.inf
;   deps\tap\amd64\tap0901.cat
;   deps\tap\amd64\tap0901.sys
;   deps\npcap\installer\npcap-x.xx.exe   (filename kept current by build.ps1)
;
; TAP driver package import command:
;   devcon.exe dp_add {app}\TAP\OemVista.inf

#define MyAppName      "L2Portal"
#define MyAppVersion   "0.2.3"
#define MyAppPublisher "L2Portal Authors"
#define MyAppExeName   "l2portal.exe"
#define MyAppDir       "{autopf}\L2Portal"

[Setup]
AppId={{A3F2C1D4-8B7E-4F6A-9C3D-1E2F5A7B4C8D}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
DefaultDirName={#MyAppDir}
; No Start Menu group needed — l2portal is a CLI tool accessed via PATH.
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
PrivilegesRequired=admin
OutputDir=..\dist
OutputBaseFilename=L2Portal-{#MyAppVersion}-Setup
Compression=lzma2/ultra
SolidCompression=yes
; 64-bit only (npcap and TAP-Windows6 are x64).
ArchitecturesInstallIn64BitMode=x64compatible
ArchitecturesAllowed=x64compatible
WizardStyle=modern
; UAC prompt at startup.
PrivilegesRequiredOverridesAllowed=commandline

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Files]
; Main executable.
Source: "..\target\release\l2portal.exe"; DestDir: "{app}"; Flags: ignoreversion

; l2p.cmd — short alias so users can type "l2p" at any prompt.
Source: "..\installer\l2p.cmd"; DestDir: "{app}"; Flags: ignoreversion

; TAP management tool (deployed alongside l2portal.exe).
Source: "..\deps\tap\tapctl.exe"; DestDir: "{app}"; Flags: ignoreversion

; TAP driver files and devcon.exe — all deployed together under {app}\TAP\.
; The installer imports the driver package into the driver store with
; "devcon.exe dp_add {app}\TAP\OemVista.inf" without creating an adapter instance.
Source: "..\deps\tap\amd64\devcon.exe";   DestDir: "{app}\TAP"; Flags: ignoreversion
Source: "..\deps\tap\amd64\OemVista.inf"; DestDir: "{app}\TAP"; Flags: ignoreversion
Source: "..\deps\tap\amd64\tap0901.cat";  DestDir: "{app}\TAP"; Flags: ignoreversion
Source: "..\deps\tap\amd64\tap0901.sys";  DestDir: "{app}\TAP"; Flags: ignoreversion

; npcap installer — filename is kept current by build.ps1 automatically.
; When running iscc manually, ensure this filename matches what is in deps/npcap/installer/.
Source: "..\deps\npcap\installer\npcap-1.87.exe"; DestDir: "{tmp}"; Flags: deleteafterinstall

; NOTE: No [Icons] section — l2portal is a CLI tool.
; Users access it via the system PATH; no Start Menu shortcuts are created.

[Registry]
; Add install dir to system PATH.
; NOTE: Removal is handled entirely in code (RemoveFromPath) at uninstall time.
; The "uninsdeletekeyifempty" flag only removes the whole key when empty and does
; NOT strip our segment from the semicolon-delimited PATH value — so we do not
; rely on it here.
Root: HKLM; Subkey: "SYSTEM\CurrentControlSet\Control\Session Manager\Environment"; \
    ValueType: expandsz; ValueName: "Path"; \
    ValueData: "{olddata};{app}"; \
    Check: NeedsAddPath(ExpandConstant('{app}')); \
    Flags: preservestringtype

[Code]
// Broadcast helper: notify other processes that environment changed
function SendMessageTimeout(hWnd: LongWord; Msg: LongWord; wParam: LongWord; lParam: string; fuFlags: LongWord; uTimeout: LongWord; var lpdwResult: LongWord): LongWord;
  external 'SendMessageTimeoutW@user32.dll stdcall';

procedure RefreshEnvironmentVariables();
var
  Dummy: LongWord;
begin
  SendMessageTimeout($FFFF, $001A, 0, 'Environment', $0002, 5000, Dummy);
end;

// ─── Helper: check if a path segment is already in PATH ────────────────────
function NeedsAddPath(Param: string): boolean;
var
  OrigPath: string;
begin
  if not RegQueryStringValue(HKEY_LOCAL_MACHINE,
    'SYSTEM\CurrentControlSet\Control\Session Manager\Environment',
    'Path', OrigPath)
  then begin
    Result := True;
    exit;
  end;
  Result := Pos(';' + Uppercase(Param) + ';', ';' + Uppercase(OrigPath) + ';') = 0;
end;

// ─── Helper: remove a path segment from the system PATH ────────────────────
// Works correctly regardless of whether the segment appears at the start,
// middle, or end of the PATH string, and handles an optional trailing backslash.
procedure RemoveFromPath(Param: string);
var
  OrigPath, NewPath, ParamUC, OrigUC: string;
  P: integer;
begin
  if not RegQueryStringValue(HKEY_LOCAL_MACHINE,
    'SYSTEM\CurrentControlSet\Control\Session Manager\Environment',
    'Path', OrigPath)
  then
    exit;

  ParamUC := Uppercase(Param);
  OrigUC  := Uppercase(OrigPath);

  // Wrap with sentinel semicolons so every entry is surrounded by ';…;'.
  OrigUC   := ';' + OrigUC + ';';
  OrigPath := ';' + OrigPath + ';';

  // Remove exact match (no trailing backslash).
  repeat
    P := Pos(';' + ParamUC + ';', OrigUC);
    if P > 0 then begin
      Delete(OrigPath, P, Length(';' + Param));
      Delete(OrigUC,   P, Length(';' + ParamUC));
    end;
  until P = 0;

  // Remove match with trailing backslash.
  repeat
    P := Pos(';' + ParamUC + '\;', OrigUC);
    if P > 0 then begin
      Delete(OrigPath, P, Length(';' + Param + '\'));
      Delete(OrigUC,   P, Length(';' + ParamUC + '\'));
    end;
  until P = 0;

  // Strip the sentinel semicolons we added.
  if (Length(OrigPath) > 0) and (OrigPath[1] = ';') then
    Delete(OrigPath, 1, 1);
  if (Length(OrigPath) > 0) and (OrigPath[Length(OrigPath)] = ';') then
    Delete(OrigPath, Length(OrigPath), 1);

  NewPath := OrigPath;
  RegWriteExpandStringValue(HKEY_LOCAL_MACHINE,
    'SYSTEM\CurrentControlSet\Control\Session Manager\Environment',
    'Path', NewPath);
end;

// ─── Install npcap if not already present ──────────────────────────────────
procedure InstallNpcapIfNeeded();
var
  NpcapKey: string;
  ResultCode: integer;
begin
  NpcapKey := 'SOFTWARE\WOW6432Node\Npcap';
  if not RegKeyExists(HKEY_LOCAL_MACHINE, NpcapKey) then begin
    Log('npcap not found — installing silently');
    Exec(ExpandConstant('{tmp}\npcap-1.87.exe'), '', '', SW_HIDE,
         ewWaitUntilTerminated, ResultCode);
    if ResultCode <> 0 then
      MsgBox('npcap installation returned code ' + IntToStr(ResultCode) +
             '. The program may not function correctly.',
             mbError, MB_OK);
  end else begin
    Log('npcap already installed — skipping');
  end;
end;

// ─── Install TAP-Windows6 driver if not already present ────────────────────
// devcon.exe, OemVista.inf, tap0901.cat and tap0901.sys
// are all deployed to {app}\TAP\.
// Command: devcon.exe dp_add {app}\TAP\OemVista.inf
procedure InstallTapDriverIfNeeded();
var
  SysDriverPath: string;
  ResultCode: integer;
  DevconPath, InfPath: string;
begin
  SysDriverPath := ExpandConstant('{sys}\drivers\tap0901.sys');
  if not FileExists(SysDriverPath) then begin
    Log('TAP driver not found — installing via devcon');
    DevconPath := ExpandConstant('{app}\TAP\devcon.exe');
    InfPath    := ExpandConstant('{app}\TAP\OemVista.inf');
    Exec(DevconPath, 'dp_add "' + InfPath + '"',
         '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
    if (ResultCode <> 0) and (ResultCode <> 1) then begin
      // devcon exits 1 when a reboot is required — that is acceptable.
      MsgBox('TAP driver installation returned code ' + IntToStr(ResultCode) +
             '. A reboot may be required, or the driver may not have installed correctly.',
             mbError, MB_OK);
    end;
  end else begin
    Log('TAP driver already installed — skipping');
  end;
end;

procedure RemoveTapDriverPackages();
var
  TempFile, PublishedName, Line: string;
  Lines: TArrayOfString;
  I, ResultCode: integer;
begin
  TempFile := ExpandConstant('{tmp}\tap-driver-packages.txt');
  if FileExists(TempFile) then
    DeleteFile(TempFile);

  if not Exec(ExpandConstant('{cmd}'),
    '/C pnputil /enum-drivers > "' + TempFile + '"',
    '', SW_HIDE, ewWaitUntilTerminated, ResultCode) then begin
    Log('Failed to execute pnputil /enum-drivers while uninstalling TAP');
    exit;
  end;
  if ResultCode <> 0 then begin
    Log('pnputil /enum-drivers returned code ' + IntToStr(ResultCode));
    exit;
  end;
  if not LoadStringsFromFile(TempFile, Lines) then begin
    Log('Failed to load TAP driver package list from ' + TempFile);
    exit;
  end;

  PublishedName := '';
  for I := 0 to GetArrayLength(Lines) - 1 do begin
    Line := Trim(Lines[I]);
    if Pos('Published Name:', Line) = 1 then begin
      PublishedName := Trim(Copy(Line, Length('Published Name:') + 1, MaxInt));
    end else if (Pos('Original Name:', Line) = 1) and
               (Lowercase(Trim(Copy(Line, Length('Original Name:') + 1, MaxInt))) = 'oemvista.inf') and
               (PublishedName <> '') then begin
      Log('Removing TAP driver package ' + PublishedName);
      Exec(ExpandConstant('{cmd}'),
        '/C pnputil /delete-driver ' + PublishedName + ' /uninstall /force',
        '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
      PublishedName := '';
    end;
  end;
end;

// ─── Custom uninstall page: optional dependency removal ────────────────────
var
  UninstNpcapCheck: TCheckBox;
  UninstTapCheck:   TCheckBox;
  UninstPage:       TWizardPage;

procedure InitializeWizard();
begin
  // IsUninstaller() returns True when this script runs as the uninstaller.
  if IsUninstaller() then begin
    // Insert our page after wpWelcome (the "confirm uninstall" page).
    // ShouldSkipPage() below ensures it is actually displayed.
    UninstPage := CreateCustomPage(
      wpWelcome,
      'Optional: Remove shared dependencies',
      'These components may also be used by other software (Wireshark, OpenVPN, etc.).' +
      ' Leave unchecked unless you are sure no other program needs them.'
    );

    UninstNpcapCheck := TCheckBox.Create(WizardForm);
    UninstNpcapCheck.Parent  := UninstPage.Surface;
    UninstNpcapCheck.Caption := 'Also uninstall Npcap  (do NOT check if Wireshark or similar is installed)';
    UninstNpcapCheck.Left    := 0;
    UninstNpcapCheck.Top     := 8;
    UninstNpcapCheck.Width   := UninstPage.SurfaceWidth;
    UninstNpcapCheck.Checked := False;

    UninstTapCheck := TCheckBox.Create(WizardForm);
    UninstTapCheck.Parent  := UninstPage.Surface;
    UninstTapCheck.Caption := 'Also uninstall TAP-Windows Adapter  (do NOT check if OpenVPN is installed)';
    UninstTapCheck.Left    := 0;
    UninstTapCheck.Top     := 32;
    UninstTapCheck.Width   := UninstPage.SurfaceWidth;
    UninstTapCheck.Checked := False;
  end;
end;

// Inno Setup's uninstaller skips custom wizard pages by default.
// Returning False here forces our dependency-removal page to be displayed.
function ShouldSkipPage(PageID: Integer): Boolean;
begin
  Result := False;
  // Never skip our custom uninstall options page.
  if IsUninstaller() and Assigned(UninstPage) and (PageID = UninstPage.ID) then
    Result := False;
end;

// ─── Post-install: run dependency installers, then show PATH notice ─────────
procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then begin
    InstallNpcapIfNeeded();
    InstallTapDriverIfNeeded();
    // Notify other processes that environment variables (PATH) may have changed.
    RefreshEnvironmentVariables();
  end;

  if CurStep = ssDone then begin
    // Tell the user the commands are now available on the PATH.
    MsgBox(
      'L2Portal has been added to the system PATH.'#13#10#13#10'You can run either of the following commands from any new terminal window:'#13#10#13#10'    l2portal --list'#13#10'    l2p --list'#13#10#13#10'Note: open a new Command Prompt or PowerShell window for the PATH change to take effect.',
      mbInformation, MB_OK);
  end;
end;

// ─── Post-uninstall: strip PATH entry, then optionally remove dependencies ──
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  UninstStr: string;
  ResultCode: integer;
begin
  if CurUninstallStep = usPostUninstall then begin
    // Always remove our install directory from the system PATH.
    RemoveFromPath(ExpandConstant('{app}'));
    // Notify other processes that the PATH has changed.
    RefreshEnvironmentVariables();

    // Uninstall npcap if the user checked the box.
    if Assigned(UninstNpcapCheck) and UninstNpcapCheck.Checked then begin
      if RegQueryStringValue(HKEY_LOCAL_MACHINE,
          'SOFTWARE\WOW6432Node\Npcap', 'UninstallString', UninstStr) then begin
        Exec(UninstStr, '/S', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
      end;
    end;

    // Remove TAP driver packages if the user checked the box.
    if Assigned(UninstTapCheck) and UninstTapCheck.Checked then begin
      RemoveTapDriverPackages();
    end;
  end;
end;
