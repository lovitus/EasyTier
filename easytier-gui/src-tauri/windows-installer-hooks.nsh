!macro stop_easytier_service
  nsExec::ExecToLog '"$SYSDIR\net.exe" stop "easytier-gui" /y'
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro stop_easytier_service
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro stop_easytier_service
!macroend
