; Tauri replacement runs the old uninstaller in place; normal uninstall runs a temp copy.
; NSIS strips _?= from $CMDLINE, so use its actual execution directory.
!macro NSIS_HOOK_PREUNINSTALL
  ${If} $EXEDIR != $INSTDIR
  ${AndIf} $UpdateMode != 1
    ClearErrors
    ExecWait '"$INSTDIR\cafe-space.exe" autostart uninstall' $0
    ${If} ${Errors}
      StrCpy $0 1
    ${EndIf}
    ${If} $0 != 0
      MessageBox MB_OK|MB_ICONSTOP "Café Space could not verify or stop its per-user daemon. Nothing else will be removed. Close the app and restart Windows before uninstalling again. If this persists, keep your data and ask the maintainer to inspect startup ownership." /SD IDOK
      SetErrorLevel 1
      Abort
    ${EndIf}
  ${EndIf}
!macroend
