; Adds "Open with Sticky Editor" to the right-click menu of every file,
; without taking over the default app of any extension.
!macro NSIS_HOOK_POSTINSTALL
  WriteRegStr HKCU "Software\Classes\*\shell\StickyEditor" "" "Open with Sticky Editor"
  WriteRegStr HKCU "Software\Classes\*\shell\StickyEditor" "Icon" "$INSTDIR\${MAINBINARYNAME}.exe"
  WriteRegStr HKCU "Software\Classes\*\shell\StickyEditor\command" "" '"$INSTDIR\${MAINBINARYNAME}.exe" "%1"'
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  DeleteRegKey HKCU "Software\Classes\*\shell\StickyEditor"
!macroend
