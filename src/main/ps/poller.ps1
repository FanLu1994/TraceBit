# TraceBit focus + idle poller.
# A persistent PowerShell process that emits JSON lines on stdout:
#   {"k":"f","t":<ms>,"app":"<label>","name":"<procname>","title":"<title>","pid":<n>,"i":<idleMs>}
#     -> on foreground app change, or title change (throttled)
#   {"k":"t","t":<ms>,"i":<idleMs>}
#     -> periodic tick (attribution / idle edge detection)
param(
  [int]$IntervalMs = 2000,
  [int]$TickMs = 30000,
  [int]$TitleThrottleMs = 8000,
  [switch]$CaptureBrowserSites
)

$ErrorActionPreference = 'SilentlyContinue'
try { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 } catch {}

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class TBWin {
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr hWnd, StringBuilder sb, int max);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);
  [DllImport("user32.dll")] public static extern bool GetLastInputInfo(ref LASTINPUTINFO plii);
  [StructLayout(LayoutKind.Sequential)]
  public struct LASTINPUTINFO { public uint cbSize; public uint dwTime; }
}
"@

if ($CaptureBrowserSites) {
  try {
    Add-Type -AssemblyName UIAutomationClient
    Add-Type -AssemblyName UIAutomationTypes
  } catch {
    [Console]::Error.WriteLine('Unable to load Windows UI Automation; browser site capture is unavailable.')
  }
}

function Get-SiteHost([string]$Value) {
  if ([string]::IsNullOrWhiteSpace($Value)) { return '' }
  $candidate = $Value.Trim()
  if ($candidate.Length -gt 4096 -or $candidate -match '\s') { return '' }
  $scheme = [regex]::Match($candidate, '^([a-zA-Z][a-zA-Z0-9+.-]*):')
  if ($scheme.Success -and $scheme.Groups[1].Value -notmatch '^(?i)https?$') { return '' }
  if (-not $scheme.Success) { $candidate = 'https://' + $candidate }

  try {
    $uri = $null
    if (-not [System.Uri]::TryCreate($candidate, [System.UriKind]::Absolute, [ref]$uri)) { return '' }
    if ($uri.Scheme -ne 'http' -and $uri.Scheme -ne 'https') { return '' }
    $hostName = $uri.IdnHost.ToLowerInvariant().TrimEnd('.')
    if ($hostName.StartsWith('www.')) { $hostName = $hostName.Substring(4) }
    if ($hostName.Length -eq 0 -or $hostName.Length -gt 253) { return '' }
    $ip = $null
    $isIp = [System.Net.IPAddress]::TryParse($hostName, [ref]$ip)
    if ($hostName -ne 'localhost' -and $hostName.IndexOf('.') -lt 0 -and -not $isIp) { return '' }
    return $hostName
  } catch { return '' }
}

function Get-BrowserSite([string]$ProcessName, [IntPtr]$WindowHandle) {
  if (-not $CaptureBrowserSites -or $WindowHandle -eq [IntPtr]::Zero) { return '' }
  $supportedBrowsers = @('chrome', 'msedge', 'brave', 'opera', 'vivaldi', 'firefox')
  if ($supportedBrowsers -notcontains $ProcessName.ToLowerInvariant()) { return '' }

  try {
    $root = [System.Windows.Automation.AutomationElement]::FromHandle($WindowHandle)
    if ($null -eq $root) { return '' }
    $condition = [System.Windows.Automation.PropertyCondition]::new(
      [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
      [System.Windows.Automation.ControlType]::Edit
    )
    $edits = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $condition)
    for ($index = 0; $index -lt $edits.Count; $index++) {
      $element = $edits.Item($index)
      $label = $element.Current.Name + ' ' + $element.Current.AutomationId
      if ($label -notmatch '(?i)(address|omnibox|search bar|地址|搜索栏|网址)') { continue }
      $pattern = $element.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern)
      if ($null -eq $pattern) { continue }
      $site = Get-SiteHost $pattern.Current.Value
      if ($site) { return $site }
    }
  } catch {}
  return ''
}

function Get-IdleMs {
  try {
    $lii = New-Object TBWin+LASTINPUTINFO
    $lii.cbSize = [System.Runtime.InteropServices.Marshal]::SizeOf([type][TBWin+LASTINPUTINFO])
    $ok = [TBWin]::GetLastInputInfo([ref]$lii)
    if (-not $ok) { return -1 }
    $now = [uint32][Environment]::TickCount
    $diff = $now - $lii.dwTime   # unsigned arithmetic survives the 49.7-day wrap
    if ($diff -gt 3600000) { $diff = 3600000 }
    return [int64]$diff
  } catch { return -1 }
}

function Escape-Json([string]$s) {
  if ($null -eq $s) { return '' }
  $sb = New-Object System.Text.StringBuilder
  foreach ($ch in $s.ToCharArray()) {
    $c = [int]$ch
    if ($c -eq 34) { [void]$sb.Append('\\"') }
    elseif ($c -eq 92) { [void]$sb.Append('\\') }
    elseif ($c -eq 10) { [void]$sb.Append('\n') }
    elseif ($c -eq 13) { [void]$sb.Append('\r') }
    elseif ($c -eq 9) { [void]$sb.Append('\t') }
    elseif ($c -lt 32) { [void]$sb.Append('\u' + $c.ToString('x4')) }
    else { [void]$sb.Append($ch) }
  }
  return $sb.ToString()
}

$procCache = @{}
$lastApp = $null
$lastTitle = $null
$lastFocusAt = 0
$lastTickAt = 0

while ($true) {
  $now = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
  $idle = Get-IdleMs

  $app = ''; $name = ''; $title = ''; $procId = 0
  $hwnd = [TBWin]::GetForegroundWindow()
  if ($hwnd -ne [IntPtr]::Zero) {
    $sb = New-Object System.Text.StringBuilder 1024
    [void][TBWin]::GetWindowText($hwnd, $sb, 1024)
    $title = $sb.ToString()
    $wpid = [uint32]0
    [void][TBWin]::GetWindowThreadProcessId($hwnd, [ref]$wpid)
    $procId = [int]$wpid
    if ($procId -ne 0) {
      if (-not $procCache.ContainsKey($procId)) {
        $pname = ''
        $p = Get-Process -Id $procId -ErrorAction SilentlyContinue
        if ($p) { $pname = $p.ProcessName }
        $label = $pname
        try {
          $pPath = $null
          try { $pPath = $p.Path } catch {}
          if ($pPath) {
            $fd = [System.Diagnostics.FileVersionInfo]::GetVersionInfo($pPath).FileDescription
            if (-not [string]::IsNullOrWhiteSpace($fd)) { $label = $fd }
          }
        } catch {}
        $procCache[$procId] = @{ label = $label; name = $pname }
        if ($procCache.Count -gt 400) { $procCache = @{} }
      }
      $app = $procCache[$procId].label
      $name = $procCache[$procId].name
    }
  }

  $site = ''
  if ($CaptureBrowserSites -and $name) {
    $site = Get-BrowserSite $name $hwnd
  }

  $appChanged = ($app -ne $lastApp)
  $titleChanged = ($title -ne $lastTitle)
  $sinceFocus = $now - $lastFocusAt

  if ($appChanged -or ($titleChanged -and $sinceFocus -ge $TitleThrottleMs)) {
    Write-Output ('{"k":"f","t":' + $now + ',"app":"' + (Escape-Json $app) + '","name":"' + (Escape-Json $name) + '","title":"' + (Escape-Json $title) + '","site":"' + (Escape-Json $site) + '","pid":' + $procId + ',"i":' + $idle + '}')
    $lastApp = $app
    $lastTitle = $title
    $lastFocusAt = $now
    $lastTickAt = $now
  } elseif (($now - $lastTickAt) -ge $TickMs) {
    Write-Output ('{"k":"t","t":' + $now + ',"site":"' + (Escape-Json $site) + '","i":' + $idle + '}')
    $lastTickAt = $now
  }

  Start-Sleep -Milliseconds $IntervalMs
}
