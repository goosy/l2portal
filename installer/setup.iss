; installer/setup.iss
; Inno Setup 6 script for L2Portal
; Requires: Inno Setup 6.x  (https://jrsoftware.org/isinfo.php)
;
; Expected layout at compile time (relative to this .iss file's parent = project root):
;   target\release\l2portal.exe
;   deps\tap\tapctl.exe
;   deps\tap\amd64\devcon.exe
;   deps\tap\amd64\OemVista.inf
;   deps\tap\amd64\tap0901.cat
;   deps\tap\amd64\tap0901.sys
;   deps\npcap\installer\npcap-1.75.exe
;
; install command:
;   devcon.exe install OemVista.inf tap0901

#define MyAppName      "L2Portal"
#define MyAppVersion   "0.1.0"
#define MyAppPublisher "L2Portal Authors"
#define MyAppExeName   "l2portal.exe"
#define MyAppDir       "{autopf}\L2Portal"

[Setup]
AppId={{A3F2C1D4-8B7E-4F6A-9C3D-1E2F5A7B4C8D}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
DefaultDirName={#MyAppDir}
DefaultGroupName={#MyAppName}
AllowNoIcons=yes
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

; TAP management tool (deployed alongside l2portal.exe).
Source: "..\deps\tap\tapctl.exe"; DestDir: "{app}"; Flags: ignoreversion

; TAP driver files and devcon.exe — all deployed together under {app}\amd64\.
; so that "devcon.exe install OemVista.inf tap0901" can find both the inf and sys files.
Source: "..\deps\tap\amd64\devcon.exe";   DestDir: "{app}\amd64"; Flags: ignoreversion
Source: "..\deps\tap\amd64\OemVista.inf"; DestDir: "{app}\amd64"; Flags: ignoreversion
Source: "..\deps\tap\amd64\tap0901.cat";  DestDir: "{app}\amd64"; Flags: ignoreversion
Source: "..\deps\tap\amd64\tap0901.sys";  DestDir: "{app}\amd64"; Flags: ignoreversion

; npcap installer (extracted to temp, run silently, then deleted).
Source: "..\deps\npcap\installer\npcap-1.75.exe"; DestDir: "{tmp}"; Flags: deleteafterinstall

[Icons]
; Start menu entry (no desktop icon by default).
Name: "{group}\{#MyAppName} Command Prompt Help"; Filename: "{sys}\cmd.exe"; \
    Parameters: "/k ""{app}\{#MyAppExeName}"" --list"; WorkingDir: "{app}"
Name: "{group}\Uninstall {#MyAppName}"; Filename: "{uninstallexe}"

[Registry]
; Add install dir to system PATH.
Root: HKLM; Subkey: "SYSTEM\CurrentControlSet\Control\Session Manager\Environment"; \
    ValueType: expandsz; ValueName: "Path"; \
    ValueData: "{olddata};{app}"; \
    Check: NeedsAddPath(ExpandConstant('{app}')); \
    Flags: preservestringtype uninsdeletekeyifempty

[Code]
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

// ─── Install npcap if not already present ──────────────────────────────────
procedure InstallNpcapIfNeeded();
var
  NpcapKey: string;
  ResultCode: integer;
begin
  NpcapKey := 'SOFTWARE\WOW6432Node\Npcap';
  if not RegKeyExists(HKEY_LOCAL_MACHINE, NpcapKey) then begin
    Log('npcap not found — installing silently');
    Exec(ExpandConstant('{tmp}\npcap-1.75.exe'), '/S', '', SW_HIDE,
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
// are all deployed to {app}\amd64\. 
// Command: devcon.exe install OemVista.inf tap0901
procedure InstallTapDriverIfNeeded();
var
  SysDriverPath: string;
  ResultCode: integer;
  DevconPath, WorkDir: string;
begin
  SysDriverPath := ExpandConstant('{sys}\drivers\tap0901.sys');
  if not FileExists(SysDriverPath) then begin
    Log('TAP driver not found — installing via devcon');
    DevconPath := ExpandConstant('{app}\amd64\devcon.exe');
    WorkDir    := ExpandConstant('{app}\amd64');
    Exec(DevconPath, 'install OemVista.inf tap0901',
         WorkDir, SW_HIDE, ewWaitUntilTerminated, ResultCode);
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

// ─── Custom uninstall page: optional dependency removal ────────────────────
var
  UninstNpcapCheck: TCheckBox;
  UninstTapCheck:   TCheckBox;
  UninstPage:       TWizardPage;

procedure InitializeWizard();
begin
  if IsUninstaller() then begin
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

// ─── Post-install: run dependency installers ───────────────────────────────
procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then begin
    InstallNpcapIfNeeded();
    InstallTapDriverIfNeeded();
  end;
end;

// ─── Post-uninstall: optionally remove dependencies ────────────────────────
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  UninstStr: string;
  ResultCode: integer;
begin
  if CurUninstallStep = usPostUninstall then begin
    // Uninstall npcap if checked.
    if Assigned(UninstNpcapCheck) and UninstNpcapCheck.Checked then begin
      if RegQueryStringValue(HKEY_LOCAL_MACHINE,
          'SOFTWARE\WOW6432Node\Npcap', 'UninstallString', UninstStr) then begin
        Exec(UninstStr, '/S', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
      end;
    end;

    // Remove TAP driver if checked.
    if Assigned(UninstTapCheck) and UninstTapCheck.Checked then begin
      Exec(ExpandConstant('{app}\amd64\devcon.exe'), 'remove tap0901', '',
           SW_HIDE, ewWaitUntilTerminated, ResultCode);
    end;
  end;
end;
