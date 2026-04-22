CMTrace Open Agent -- Operator README
======================================

Installation paths
------------------
  Binary:       %ProgramFiles%\CMTraceOpen\Agent\cmtraceopen-agent.exe
  Config:       %ProgramData%\CMTraceOpen\Agent\config.toml
  Upload queue: %ProgramData%\CMTraceOpen\Agent\Queue\
  Agent logs:   %ProgramData%\CMTraceOpen\Agent\logs\

Service management
------------------
  Get-Service CMTraceOpenAgent              # check status
  Restart-Service CMTraceOpenAgent          # restart after config edit
  Stop-Service CMTraceOpenAgent             # stop (does not uninstall)

Configuration
-------------
  Edit %ProgramData%\CMTraceOpen\Agent\config.toml then restart the service.
  Config survives MSI upgrade. To reset to defaults, delete config.toml and
  run MSI repair:
    msiexec /fa CMTraceOpenAgent-<version>.msi

Uninstall
---------
  msiexec /x {ProductCode} /qn                     # remove binary; keep config + queue
  msiexec /x {ProductCode} KEEP_USER_DATA=0 /qn    # full purge of %ProgramData% too

MSI install log
---------------
  msiexec /i CMTraceOpenAgent-<version>.msi /qn /l*v "%TEMP%\cmtrace-install.log"
  The log contains the Cloud PKI cert-check result and service-start outcome.
  Search the log for "CertCheck" or "WARN:" to find relevant entries.

Support
-------
  https://github.com/adamgell/cmtraceopen-web
