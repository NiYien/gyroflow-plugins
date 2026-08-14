$ErrorActionPreference = 'Stop'

$scriptsDir = Split-Path -Parent $PSScriptRoot
$repoRoot = Split-Path -Parent $scriptsDir
$clipEntry = Join-Path $scriptsDir 'Gyroflow NiYien Auto Cut Current Clip.lua'
$trackEntry = Join-Path $scriptsDir 'Gyroflow NiYien Auto Cut Current Track.lua'
$commonModule = Join-Path $scriptsDir 'gyroflow_autocut_common.inc'
$legacyEntry = Join-Path $scriptsDir 'Gyroflow NiYien Auto Cut.lua'

function Assert-True([bool]$condition, [string]$message) {
    if (-not $condition) {
        throw $message
    }
}

Assert-True (Test-Path -LiteralPath $clipEntry) 'Current Clip entry is missing'
Assert-True (Test-Path -LiteralPath $trackEntry) 'Current Track entry is missing'
Assert-True (Test-Path -LiteralPath $commonModule) 'Shared auto-cut module is missing'
Assert-True (-not (Test-Path -LiteralPath $legacyEntry)) 'Legacy ambiguous menu entry must be removed'

$clipText = Get-Content -LiteralPath $clipEntry -Raw
$trackText = Get-Content -LiteralPath $trackEntry -Raw
$commonText = Get-Content -LiteralPath $commonModule -Raw

Assert-True ($clipText -match 'gyroflow_autocut_common\.inc') 'Current Clip entry must load the shared module'
Assert-True ($clipText -match 'run\("clip"\)') 'Current Clip entry must select clip mode'
Assert-True ($trackText -match 'gyroflow_autocut_common\.inc') 'Current Track entry must load the shared module'
Assert-True ($trackText -match 'run\("track"\)') 'Current Track entry must select track mode'

Assert-True ($commonText -match 'GetCurrentVideoItem') 'Shared module must anchor both modes at the playhead item'
Assert-True ($commonText -match 'GetItemListInTrack') 'Track mode must enumerate the current video track'
Assert-True ($commonText -match 'mediaType\s*=\s*1') 'Video must be appended explicitly'
Assert-True ($commonText -match 'mediaType\s*=\s*2') 'Audio must be appended explicitly'
Assert-True ($commonText -match 'SetClipsLinked') 'Rebuilt video and audio must be linked explicitly'
Assert-True ($commonText -match 'validate_linked_audio') 'Audio presence and links must be validated before staging cleanup'

$openfxJustfile = Get-Content -LiteralPath (Join-Path $repoRoot 'openfx\Justfile') -Raw
$adobeJustfile = Get-Content -LiteralPath (Join-Path $repoRoot 'adobe\Justfile') -Raw
Assert-True ($openfxJustfile -match 'ResolveScripts') 'OpenFX packaging must include ResolveScripts'
Assert-True ($adobeJustfile -notmatch 'ResolveScripts|resolve-scripts') 'Adobe packaging must remain Resolve-sidecar free'

Write-Output 'Resolve auto-cut script contract checks passed.'
