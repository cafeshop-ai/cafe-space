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
      MessageBox MB_OK|MB_ICONSTOP "无法停止 Café Space 守护进程，已取消卸载。请关闭 Café Space，重启电脑后再卸载。" /SD IDOK
      SetErrorLevel 1
      Abort
    ${EndIf}
  ${EndIf}
!macroend
