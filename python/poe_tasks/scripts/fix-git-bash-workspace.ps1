Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Resolve-GitBashPath {
  $candidates = [System.Collections.Generic.List[string]]::new()

  $gitCmd = Get-Command git -ErrorAction SilentlyContinue
  if ($null -ne $gitCmd) {
    $gitExeDir = Split-Path -Parent $gitCmd.Source
    $gitRoot = Split-Path -Parent $gitExeDir
    $candidates.Add((Join-Path $gitRoot "bin\bash.exe"))
  }

  if ($env:ProgramFiles) {
    $candidates.Add((Join-Path $env:ProgramFiles "Git\bin\bash.exe"))
  }

  if ($env:ProgramW6432) {
    $candidates.Add((Join-Path $env:ProgramW6432 "Git\bin\bash.exe"))
  }

  if ($env:ProgramFiles -and ${env:ProgramFiles(x86)}) {
    $candidates.Add((Join-Path ${env:ProgramFiles(x86)} "Git\bin\bash.exe"))
  }

  $uniqueCandidates = $candidates | Select-Object -Unique
  foreach ($candidate in $uniqueCandidates) {
    if ([string]::IsNullOrWhiteSpace($candidate)) {
      continue
    }

    if (Test-Path -LiteralPath $candidate -PathType Leaf) {
      return (Resolve-Path -LiteralPath $candidate).Path
    }
  }

  throw "Unable to find Git Bash (bash.exe). Install Git for Windows first."
}

function To-Hashtable($value) {
  if ($null -eq $value) {
    return $null
  }

  if ($value -is [System.Collections.IDictionary]) {
    $result = @{}
    foreach ($key in $value.Keys) {
      $result[$key] = To-Hashtable $value[$key]
    }

    return $result
  }

  if ($value -is [System.Collections.IEnumerable] -and -not ($value -is [string])) {
    $result = @()
    foreach ($item in $value) {
      $result += ,(To-Hashtable $item)
    }

    return $result
  }

  if ($value -is [pscustomobject]) {
    $result = @{}
    foreach ($prop in $value.PSObject.Properties) {
      $result[$prop.Name] = To-Hashtable $prop.Value
    }

    return $result
  }

  return $value
}

if ($env:OS -ne "Windows_NT") {
  throw "This task is Windows-only."
}

$workspaceRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path
$settingsDir = Join-Path $workspaceRoot ".vscode"
$settingsPath = Join-Path $settingsDir "settings.json"

$gitBinDir = Split-Path -Parent (Resolve-GitBashPath)
$workspacePathPrefix = $gitBinDir + ';${env:PATH}'

$settings = @{}
if (Test-Path -LiteralPath $settingsPath -PathType Leaf) {
  $raw = Get-Content -LiteralPath $settingsPath -Raw
  if (-not [string]::IsNullOrWhiteSpace($raw)) {
    $parsed = ConvertFrom-Json -InputObject $raw
    if ($null -ne $parsed) {
      $settings = To-Hashtable $parsed
    }
  }
}

if (-not ($settings.ContainsKey("terminal.integrated.env.windows"))) {
  $settings["terminal.integrated.env.windows"] = @{}
}

if (-not ($settings["terminal.integrated.env.windows"] -is [System.Collections.IDictionary])) {
  $settings["terminal.integrated.env.windows"] = @{}
}

$settings["terminal.integrated.env.windows"]["PATH"] = $workspacePathPrefix

New-Item -ItemType Directory -Path $settingsDir -Force | Out-Null
$settings | ConvertTo-Json -Depth 100 | Set-Content -LiteralPath $settingsPath -Encoding utf8

Write-Host "Workspace terminal PATH override configured."
Write-Host "Workspace: $workspaceRoot"
Write-Host "Settings file: $settingsPath"
Write-Host "Git Bash bin: $gitBinDir"
Write-Host "Set terminal.integrated.env.windows.PATH = $workspacePathPrefix"
Write-Host "Open a new terminal in this workspace to use the updated environment."
