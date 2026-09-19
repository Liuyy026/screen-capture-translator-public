[CmdletBinding()]
param(
    [switch]$Apply,
    [string]$Source,
    [string]$Target
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if (-not $Source) {
    $Source = Join-Path (Split-Path -Parent $PSScriptRoot) '.models\ollama'
}
if (-not $Target) {
    $Target = Join-Path ([Environment]::GetFolderPath('UserProfile')) '.ollama\models'
}

function Resolve-FullPath([string]$PathValue) {
    return [IO.Path]::GetFullPath($PathValue)
}

$sourcePath = Resolve-FullPath $Source
$targetPath = Resolve-FullPath $Target

if (-not (Test-Path -LiteralPath $sourcePath -PathType Container)) {
    throw "Source Ollama store does not exist: $sourcePath"
}
$sourceItem = Get-Item -LiteralPath $sourcePath -Force
if ($sourceItem.LinkType -match 'Junction') {
    $linkTargets = @($sourceItem.Target | ForEach-Object { Resolve-FullPath $_ })
    if ($targetPath -in $linkTargets -and
        (Test-Path -LiteralPath (Join-Path $sourcePath 'blobs') -PathType Container) -and
        (Test-Path -LiteralPath (Join-Path $sourcePath 'manifests') -PathType Container)) {
        Write-Host "Migration already complete. Junction: $sourcePath -> $targetPath"
        exit 0
    }
    throw "Source is already a link to a different location: $sourcePath"
}
if (-not (Test-Path -LiteralPath (Join-Path $sourcePath 'blobs') -PathType Container) -or
    -not (Test-Path -LiteralPath (Join-Path $sourcePath 'manifests') -PathType Container)) {
    throw "Source is not a complete Ollama model store: $sourcePath"
}
if ($sourcePath -eq $targetPath) {
    throw 'Source and target must be different paths.'
}
$sourcePrefix = $sourcePath.TrimEnd('\') + '\'
$targetPrefix = $targetPath.TrimEnd('\') + '\'
if ($sourcePath.StartsWith($targetPrefix, [StringComparison]::OrdinalIgnoreCase) -or
    $targetPath.StartsWith($sourcePrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'Source and target cannot contain one another.'
}

$listeners = @(Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue |
    Where-Object { $_.LocalPort -in @(11434, 11439) })
if ($Apply -and $listeners.Count -gt 0) {
    $ports = ($listeners | Select-Object -ExpandProperty LocalPort -Unique) -join ', '
    throw "Ollama is listening on port(s) $ports. Exit Ollama and the translation software first."
}

$targetScaffolding = @()
if (Test-Path -LiteralPath $targetPath) {
    $targetItem = Get-Item -LiteralPath $targetPath -Force
    if ($targetItem.LinkType) {
        throw "Target must be a real directory, not a link: $targetPath"
    }
    $targetChildren = @(Get-ChildItem -LiteralPath $targetPath -Force)
    $targetScaffolding = @($targetChildren | Where-Object {
        $_.PSIsContainer -and $_.Name -in @('blobs', 'manifests') -and
        @(Get-ChildItem -LiteralPath $_.FullName -Force).Count -eq 0
    })
    if (@($targetChildren | Where-Object { $_ -notin $targetScaffolding }).Count -gt 0) {
        throw "Target contains model data or unexpected files; no files were changed: $targetPath"
    }
}

$sourceChildren = @(Get-ChildItem -LiteralPath $sourcePath -Force)
$sourceBytes = ($sourceChildren | ForEach-Object {
    if ($_.PSIsContainer) {
        (Get-ChildItem -LiteralPath $_.FullName -Recurse -Force -File -ErrorAction Stop |
            Measure-Object -Property Length -Sum).Sum
    } else { $_.Length }
} | Measure-Object -Sum).Sum

Write-Host "Source: $sourcePath"
Write-Host "Target: $targetPath"
Write-Host ("Data to move: {0:N2} GiB" -f ($sourceBytes / 1GB))
Write-Host 'Action: move the complete blobs/manifests store, then create a junction at the source path.'

if (-not $Apply) {
    if ($listeners.Count -gt 0) {
        $ports = ($listeners | Select-Object -ExpandProperty LocalPort -Unique) -join ', '
        Write-Warning "Ollama is currently listening on port(s) $ports. Close it before using -Apply."
    }
    Write-Host 'Preview only. Re-run with -Apply after closing Ollama and the translation software.'
    exit 0
}

if (Test-Path -LiteralPath $targetPath) {
    Remove-Item -LiteralPath $targetPath -Recurse -Force
} else {
    New-Item -ItemType Directory -Path (Split-Path -Parent $targetPath) -Force | Out-Null
}
Move-Item -LiteralPath $sourcePath -Destination $targetPath
try {
    New-Item -ItemType Junction -Path $sourcePath -Target $targetPath | Out-Null
    $link = Get-Item -LiteralPath $sourcePath -Force
    if ($link.LinkType -notmatch 'Junction') {
        throw "Junction validation failed: $sourcePath"
    }
    if (-not (Test-Path -LiteralPath (Join-Path $sourcePath 'blobs') -PathType Container) -or
        -not (Test-Path -LiteralPath (Join-Path $sourcePath 'manifests') -PathType Container)) {
        throw 'Junction exists but the complete model store is not visible through it.'
    }
} catch {
    if (Test-Path -LiteralPath $sourcePath) {
        $failedSource = Get-Item -LiteralPath $sourcePath -Force
        if ($failedSource.LinkType) {
            Remove-Item -LiteralPath $sourcePath -Force
        }
    }
    if (-not (Test-Path -LiteralPath $sourcePath) -and
        (Test-Path -LiteralPath $targetPath -PathType Container)) {
        Move-Item -LiteralPath $targetPath -Destination $sourcePath
    }
    throw
}
Write-Host "Migration complete. Junction: $sourcePath -> $targetPath"
