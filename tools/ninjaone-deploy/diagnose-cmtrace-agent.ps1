<#
.SYNOPSIS
    Full-surface diagnostic for cmtraceopen-agent on a Windows endpoint.

.DESCRIPTION
    One-shot sweep that dumps everything we've ever needed to debug an
    agent that installed but isn't uploading. No surgery — read-only.
    Run under SYSTEM via NinjaOne (or locally as admin) and paste the
    output into the thread.

    Sections (in order):
      01  host identity + OS + uptime + time-sync
      02  cmtraceopen-agent service state + PID + process details
      03  cmtraceopen-agent installed version (Uninstall registry)
      04  effective config.toml (both candidate locations)
      05  config-state.json (config-sync marker + rollback state)
      06  ProgramData layout — both casing variants, recursive with sizes
      07  queue bundles (zips + sidecars) — full JSON of recent sidecars
      08  agent log files — list + tail
      09  MSI install log tail
      10  recent Windows Application event log — our service + MsiInstaller
      11  recent System event log — SCM stop/start of the service
      12  network — TCP reach, /healthz, DNS, firewall profile
      13  machine cert store (LocalMachine\My) — future mTLS sanity
      14  MDM enrollment / device identity bits
      15  SC queryex + service config

    Non-fatal: every section swallows its own errors so a single bad call
    doesn't kill the rest of the dump.
#>

[CmdletBinding()]
param(
    [int]$LogTail          = 120,
    [int]$EventWindowHours = 4,
    [int]$SidecarCount     = 20
)

$ErrorActionPreference = 'Continue'

function Section($title) {
    Write-Host ''
    Write-Host ('=' * 78)
    Write-Host "== $title"
    Write-Host ('=' * 78)
}
function Sub($title) { Write-Host ''; Write-Host "-- $title" }
function Safe($scriptblock) {
    try { & $scriptblock } catch { Write-Host "  [!] error: $($_.Exception.Message)" -ForegroundColor Yellow }
}

$AgentDirCandidates = @(
    'C:\ProgramData\CMTraceOpen\Agent',
    'C:\ProgramData\cmtraceopen-agent'
)

# ---------------------------------------------------------------------- 01
Section '01  host identity + OS + uptime'
Safe {
    $os = Get-CimInstance Win32_OperatingSystem
    [pscustomobject]@{
        ComputerName = $env:COMPUTERNAME
        UserName     = "$env:USERDOMAIN\$env:USERNAME"
        OS           = $os.Caption
        Version      = $os.Version
        Build        = $os.BuildNumber
        InstallDate  = $os.InstallDate
        LastBoot     = $os.LastBootUpTime
        UptimeHours  = [math]::Round(((Get-Date) - $os.LastBootUpTime).TotalHours, 2)
        TimeZone     = (Get-TimeZone).Id
        LocalTime    = (Get-Date).ToString('o')
        UtcNow       = (Get-Date).ToUniversalTime().ToString('o')
        PSVersion    = $PSVersionTable.PSVersion.ToString()
    } | Format-List
}
Sub 'time sync (w32tm /query /status)'
Safe { & w32tm.exe /query /status 2>&1 | Write-Host }

# ---------------------------------------------------------------------- 02
Section '02  service state + process details'
$svc = Get-Service -Name 'CMTraceOpenAgent' -ErrorAction SilentlyContinue
if (-not $svc) {
    Write-Host '  CMTraceOpenAgent service NOT FOUND' -ForegroundColor Red
} else {
    Safe { $svc | Format-List Name, DisplayName, Status, StartType, CanStop, ServicesDependedOn }
    $row = & sc.exe queryex CMTraceOpenAgent 2>$null | Select-String 'PID\s*:\s*(\d+)'
    if ($row) {
        $svcPid = [int]$row.Matches[0].Groups[1].Value
        Safe {
            $p = Get-Process -Id $svcPid -ErrorAction Stop
            [pscustomobject]@{
                PID           = $p.Id
                Path          = $p.Path
                StartTime     = $p.StartTime
                RunTimeMin    = [math]::Round(((Get-Date) - $p.StartTime).TotalMinutes, 1)
                CPU_Sec       = [math]::Round($p.CPU, 2)
                WorkingSetMB  = [math]::Round($p.WorkingSet64 / 1MB, 1)
                Threads       = $p.Threads.Count
                Handles       = $p.HandleCount
            } | Format-List
        }
        Sub 'TCP connections held by agent PID'
        Safe {
            Get-NetTCPConnection -OwningProcess $svcPid -ErrorAction SilentlyContinue |
                Select-Object State, LocalAddress, LocalPort, RemoteAddress, RemotePort |
                Format-Table -AutoSize
        }
    }
}

Sub 'sc.exe qc (service config)'
Safe { & sc.exe qc CMTraceOpenAgent 2>&1 | Write-Host }

Sub 'sc.exe queryex (full)'
Safe { & sc.exe queryex CMTraceOpenAgent 2>&1 | Write-Host }

# ---------------------------------------------------------------------- 03
Section '03  installed version (Uninstall registry)'
foreach ($p in @(
        'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*',
        'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*'
    )) {
    Safe {
        Get-ItemProperty -Path $p -ErrorAction SilentlyContinue |
            Where-Object { $_.PSObject.Properties['DisplayName'] -and $_.DisplayName -like 'CMTrace Open Agent*' } |
            Select-Object DisplayName, DisplayVersion, Publisher, InstallDate, InstallSource, UninstallString |
            Format-List
    }
}

# ---------------------------------------------------------------------- 04
Section '04  effective config.toml'
foreach ($dir in $AgentDirCandidates) {
    $cfg = Join-Path $dir 'config.toml'
    Sub $cfg
    if (Test-Path -LiteralPath $cfg) {
        Safe { Get-Content -LiteralPath $cfg -Raw | Write-Host }
    } else {
        Write-Host '  (not present)'
    }
}

# ---------------------------------------------------------------------- 05
Section '05  config-state.json (config-sync marker)'
foreach ($dir in $AgentDirCandidates) {
    $cs = Join-Path $dir 'config-state.json'
    Sub $cs
    if (Test-Path -LiteralPath $cs) {
        Safe { Get-Content -LiteralPath $cs -Raw | Write-Host }
    } else {
        Write-Host '  (not present)'
    }
}

# ---------------------------------------------------------------------- 06
Section '06  ProgramData tree — full recursive listing'
foreach ($dir in $AgentDirCandidates) {
    Sub $dir
    if (Test-Path -LiteralPath $dir) {
        Safe {
            Get-ChildItem -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue |
                Select-Object @{N='RelPath';E={$_.FullName.Substring($dir.Length)}},
                              @{N='Size';E={ if ($_.PSIsContainer) { '<dir>' } else { $_.Length } }},
                              LastWriteTime |
                Sort-Object RelPath |
                Format-Table -AutoSize
        }
    } else {
        Write-Host '  (not present)'
    }
}

# ---------------------------------------------------------------------- 07
Section '07  queue bundles + sidecar JSON'
foreach ($dir in $AgentDirCandidates) {
    $q = Join-Path $dir 'queue'
    Sub $q
    if (-not (Test-Path -LiteralPath $q)) {
        Write-Host '  (not present)'
        continue
    }
    Safe {
        $all = Get-ChildItem -LiteralPath $q -File -Recurse -ErrorAction SilentlyContinue
        Write-Host "  total files: $($all.Count)"
        Write-Host "  zip count  : $(@($all | Where-Object {$_.Extension -eq '.zip'}).Count)"
        Write-Host "  json count : $(@($all | Where-Object {$_.Extension -eq '.json'}).Count)"
        Write-Host "  total MB   : $([math]::Round(($all | Measure-Object Length -Sum).Sum / 1MB, 1))"
    }
    Sub 'recent sidecars (full JSON)'
    Safe {
        $sidecars = Get-ChildItem -LiteralPath $q -Filter '*.json' -File -ErrorAction SilentlyContinue |
            Sort-Object LastWriteTime -Descending |
            Select-Object -First $SidecarCount
        foreach ($f in $sidecars) {
            Write-Host "::: $($f.Name)  ($(Get-Date $f.LastWriteTime -Format 's'))"
            Get-Content -LiteralPath $f.FullName -Raw | Write-Host
            Write-Host ''
        }
    }
}

# ---------------------------------------------------------------------- 08
Section '08  agent log files'
foreach ($dir in $AgentDirCandidates) {
    foreach ($sub in @('logs', 'log')) {
        $lp = Join-Path $dir $sub
        if (-not (Test-Path -LiteralPath $lp)) { continue }
        Sub $lp
        Safe {
            $files = Get-ChildItem -LiteralPath $lp -File -ErrorAction SilentlyContinue |
                Sort-Object LastWriteTime -Descending
            if (-not $files) { Write-Host '  (empty)'; return }
            $files | Select-Object Name, Length, LastWriteTime | Format-Table -AutoSize
            $top = $files | Select-Object -First 1
            Write-Host ''
            Write-Host "  tail -$LogTail $($top.Name):"
            Get-Content -LiteralPath $top.FullName -Tail $LogTail | ForEach-Object { Write-Host "    $_" }
        }
    }
}

# ---------------------------------------------------------------------- 09
Section '09  MSI install log tail'
$msilog = Join-Path $env:WINDIR 'Temp\CMTraceOpenAgent-install.log'
if (Test-Path -LiteralPath $msilog) {
    Safe {
        Write-Host "path: $msilog"
        Get-Content -LiteralPath $msilog -Tail 60 | ForEach-Object { Write-Host "  $_" }
    }
} else {
    Write-Host '  (no install log — agent may have been installed via Intune IME which keeps its own log)'
}

Sub 'Intune IME Win32 logs (tail of most recent AgentExecutor.log)'
Safe {
    $ime = 'C:\ProgramData\Microsoft\IntuneManagementExtension\Logs'
    if (Test-Path -LiteralPath $ime) {
        $f = Get-ChildItem -LiteralPath $ime -Filter 'AgentExecutor*.log' -File -ErrorAction SilentlyContinue |
            Sort-Object LastWriteTime -Descending | Select-Object -First 1
        if ($f) {
            Write-Host "path: $($f.FullName)"
            Get-Content -LiteralPath $f.FullName -Tail 30 | ForEach-Object { Write-Host "  $_" }
        } else { Write-Host '  (no AgentExecutor log found)' }
    } else { Write-Host '  (IME path not present)' }
}

# ---------------------------------------------------------------------- 10
Section '10  Windows Application event log (last ${EventWindowHours}h)'
Safe {
    Get-WinEvent -FilterHashtable @{
        LogName   = 'Application'
        StartTime = (Get-Date).AddHours(-$EventWindowHours)
    } -MaxEvents 200 -ErrorAction SilentlyContinue |
    Where-Object {
        $_.ProviderName -match 'cmtrace|MsiInstaller' -or
        $_.Message      -match 'CMTraceOpen'
    } |
    Select-Object TimeCreated, LevelDisplayName, ProviderName, Id,
                  @{N='Msg';E={ ($_.Message -replace "\r?\n", ' ').Substring(0, [Math]::Min(220, $_.Message.Length)) }} |
    Format-Table -AutoSize -Wrap
}

# ---------------------------------------------------------------------- 11
Section '11  System event log — SCM for CMTraceOpenAgent (last ${EventWindowHours}h)'
Safe {
    Get-WinEvent -FilterHashtable @{
        LogName   = 'System'
        StartTime = (Get-Date).AddHours(-$EventWindowHours)
        ProviderName = 'Service Control Manager'
    } -MaxEvents 300 -ErrorAction SilentlyContinue |
    Where-Object { $_.Message -match 'CMTraceOpen' } |
    Select-Object TimeCreated, LevelDisplayName, Id,
                  @{N='Msg';E={ ($_.Message -replace "\r?\n", ' ').Substring(0, [Math]::Min(220, $_.Message.Length)) }} |
    Format-Table -AutoSize -Wrap
}

# ---------------------------------------------------------------------- 12
Section '12  network — reach api-server'
$endpoint = $null
foreach ($dir in $AgentDirCandidates) {
    $cfg = Join-Path $dir 'config.toml'
    if (Test-Path -LiteralPath $cfg) {
        $m = Select-String -LiteralPath $cfg -Pattern '^\s*api_endpoint\s*=\s*"([^"]+)"' | Select-Object -First 1
        if ($m) { $endpoint = $m.Matches[0].Groups[1].Value; break }
    }
}
if (-not $endpoint) {
    Write-Host '  (endpoint not parseable from config.toml — skipping network probe)'
} else {
    $u = [uri]$endpoint
    $port = if ($u.Port -gt 0) { $u.Port } elseif ($u.Scheme -eq 'https') { 443 } else { 80 }
    Write-Host "  endpoint: $endpoint  host=$($u.Host)  port=$port  scheme=$($u.Scheme)"

    Sub 'DNS'
    Safe { Resolve-DnsName -Name $u.Host -ErrorAction SilentlyContinue | Select-Object Name, IPAddress, Type | Format-Table -AutoSize }

    Sub 'Test-NetConnection (TCP reach, detailed)'
    Safe {
        $r = Test-NetConnection -ComputerName $u.Host -Port $port -InformationLevel Detailed -WarningAction SilentlyContinue
        [pscustomobject]@{
            TcpTestSucceeded = $r.TcpTestSucceeded
            PingSucceeded    = $r.PingSucceeded
            PingReplyDetails = ($r.PingReplyDetails | Out-String).Trim()
            SourceAddress    = $r.SourceAddress.IPAddress
            RemoteAddress    = $r.RemoteAddress
            InterfaceAlias   = $r.InterfaceAlias
            NetRoute         = ($r.NetRoute.DestinationPrefix | Select-Object -First 1)
        } | Format-List
    }

    Sub '/healthz'
    Safe {
        $resp = Invoke-WebRequest -Uri ("{0}://{1}:{2}/healthz" -f $u.Scheme, $u.Host, $port) -UseBasicParsing -TimeoutSec 5
        Write-Host "  HTTP $($resp.StatusCode)  $([int]($resp.Content.Length))B body"
    }

    Sub 'POST /v1/ingest/bundles (auth probe — expect a 4xx, not a timeout)'
    # Sends an intentionally-empty POST just to see the auth/routing response.
    # A 401 or 400 here proves the agent would be able to reach the route; a
    # timeout or connection-refused proves the network path is dead.
    Safe {
        $probe = try {
            Invoke-WebRequest -Uri ("{0}://{1}:{2}/v1/ingest/bundles" -f $u.Scheme, $u.Host, $port) `
                -Method POST -Body '{}' -ContentType 'application/json' -UseBasicParsing -TimeoutSec 5 `
                -ErrorAction Stop
        } catch [System.Net.WebException] {
            $_.Exception.Response
        } catch { $_ }
        if ($probe -is [System.Net.HttpWebResponse]) {
            Write-Host "  HTTP $([int]$probe.StatusCode)  $($probe.StatusDescription)"
        } elseif ($probe -is [Microsoft.PowerShell.Commands.WebResponseObject]) {
            Write-Host "  HTTP $($probe.StatusCode)"
        } else {
            Write-Host "  probe result: $probe"
        }
    }
}

Sub 'Windows firewall profile (is anything blocking outbound?)'
Safe { Get-NetFirewallProfile | Select-Object Name, Enabled, DefaultOutboundAction | Format-Table -AutoSize }

# ---------------------------------------------------------------------- 13
Section '13  LocalMachine\My cert store (future mTLS)'
Safe {
    Get-ChildItem Cert:\LocalMachine\My -ErrorAction SilentlyContinue |
        Select-Object Subject, Issuer, NotAfter, Thumbprint, HasPrivateKey |
        Format-Table -AutoSize -Wrap
}

# ---------------------------------------------------------------------- 14
Section '14  MDM enrollment / device identity'
Safe { & dsregcmd.exe /status 2>&1 | Select-String -Pattern 'AzureAdJoined|DomainJoined|DeviceName|DeviceId|EnterpriseJoined|MdmUrl|WorkplaceJoined' | ForEach-Object { Write-Host "  $($_.Line.Trim())" } }

# ---------------------------------------------------------------------- 15
Section '15  done'
Write-Host "  hostname : $env:COMPUTERNAME"
Write-Host "  local    : $(Get-Date -Format 's')"
Write-Host "  utc      : $((Get-Date).ToUniversalTime().ToString('s'))Z"
