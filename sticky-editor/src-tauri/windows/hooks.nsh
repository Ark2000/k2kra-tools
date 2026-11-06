!define RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"
!define APP_KEY "Software\Sticky Editor"

!macro NSIS_HOOK_POSTINSTALL
  ; "Open with Sticky Editor" on every file, without taking over any
  ; extension's default app.
  WriteRegStr HKCU "Software\Classes\*\shell\StickyEditor" "" "Open with Sticky Editor"
  WriteRegStr HKCU "Software\Classes\*\shell\StickyEditor" "Icon" "$INSTDIR\${MAINBINARYNAME}.exe"
  WriteRegStr HKCU "Software\Classes\*\shell\StickyEditor\command" "" '"$INSTDIR\${MAINBINARYNAME}.exe" "%1"'

  ; On by default, once: an upgrade must not undo the user turning it off.
  ReadRegStr $0 HKCU "${APP_KEY}" "AutostartSeeded"
  StrCmp $0 "1" +3
    WriteRegStr HKCU "${RUN_KEY}" "Sticky Editor" '"$INSTDIR\${MAINBINARYNAME}.exe" --restore'
    WriteRegStr HKCU "${APP_KEY}" "AutostartSeeded" "1"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  DeleteRegKey HKCU "Software\Classes\*\shell\StickyEditor"
  DeleteRegValue HKCU "${RUN_KEY}" "Sticky Editor"
  DeleteRegKey HKCU "${APP_KEY}"
!macroend
