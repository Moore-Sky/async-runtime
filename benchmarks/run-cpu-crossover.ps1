<#
Runs the CPU-crossover decision benchmark without retaining Criterion's HTML,
plots, or cache.  Each invocation needs a fresh RunId; formal runs always use
five independent cargo-bench processes and a balanced case order.

Examples:
  .\benchmarks\run-cpu-crossover.ps1 -Mode Rapid -RunId 20260907-rapid
  .\benchmarks\run-cpu-crossover.ps1 -Mode Formal -RunId 20260907-formal -Workloads 20us,50us,100us
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [ValidatePattern('^[A-Za-z0-9][A-Za-z0-9._-]*$')]
    [string]$RunId,

    [ValidateSet('Rapid', 'Formal')]
    [string]$Mode = 'Rapid',

    [string[]]$Workloads = @('0us', '2us', '5us', '10us', '20us', '50us', '100us', '200us'),

    [string]$OutputRoot = (Join-Path $PSScriptRoot '..\target\cpu-crossover-runs')
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Invoke-ExternalText([string]$File, [string[]]$Arguments) {
    $value = & $File @Arguments 2>$null
    if ($LASTEXITCODE -ne 0) { return "<unavailable: $File>" }
    return ($value | Out-String).Trim()
}

function Get-Median([double[]]$Values) {
    $ordered = @($Values | Sort-Object)
    $middle = [int]($ordered.Count / 2)
    if (($ordered.Count % 2) -eq 1) { return $ordered[$middle] }
    return ($ordered[$middle - 1] + $ordered[$middle]) / 2.0
}

function Copy-CriterionRaw(
    [string]$CriterionRoot,
    [string]$RawRound,
    [string[]]$ExpectedWorkloads,
    [string[]]$ExpectedCases
) {
    $benchmarks = Get-ChildItem -LiteralPath $CriterionRoot -Recurse -File -Filter 'benchmark.json' |
        Where-Object {
            if ($_.Directory.Name -ne 'new') { return $false }
            $metadata = Get-Content -Raw -LiteralPath $_.FullName | ConvertFrom-Json
            $parts = ([string]$metadata.full_id) -split '/'
            return $parts.Count -eq 4 -and
                $parts[0] -eq 'cpu_crossover' -and
                $ExpectedWorkloads -contains $parts[2] -and
                $ExpectedCases -contains $parts[3]
        }
    $expectedCount = $ExpectedWorkloads.Count * $ExpectedCases.Count
    if ($benchmarks.Count -ne $expectedCount) {
        throw "Criterion case count mismatch below ${CriterionRoot}: expected=$expectedCount actual=$($benchmarks.Count)"
    }

    foreach ($benchmark in $benchmarks) {
        $sourceDirectory = $benchmark.Directory
        $relative = $sourceDirectory.Parent.FullName.Substring($CriterionRoot.Length).TrimStart('\', '/')
        $destination = Join-Path $RawRound $relative
        New-Item -ItemType Directory -Force -Path $destination | Out-Null
        foreach ($name in @('sample.json', 'estimates.json', 'benchmark.json')) {
            $source = Join-Path $sourceDirectory.FullName $name
            if (-not (Test-Path -LiteralPath $source)) { throw "Criterion output is missing $source" }
            Copy-Item -LiteralPath $source -Destination (Join-Path $destination $name) -Force
        }
    }
}

function Read-Cases([string]$RawRoot) {
    $rows = New-Object System.Collections.Generic.List[object]
    $benchmarkFiles = Get-ChildItem -LiteralPath $RawRoot -Recurse -File -Filter 'benchmark.json'
    foreach ($benchmarkFile in $benchmarkFiles) {
        $estimatesFile = Join-Path $benchmarkFile.Directory.FullName 'estimates.json'
        if (-not (Test-Path -LiteralPath $estimatesFile)) { continue }
        $benchmark = Get-Content -Raw -LiteralPath $benchmarkFile.FullName | ConvertFrom-Json
        $estimates = Get-Content -Raw -LiteralPath $estimatesFile | ConvertFrom-Json
        $id = [string]$benchmark.full_id
        $parts = $id -split '/'
        if ($parts.Count -lt 4 -or $parts[0] -ne 'cpu_crossover') { continue }
        $rawRelative = $benchmarkFile.FullName.Substring($RawRoot.Length).TrimStart('\', '/')
        $round = ($rawRelative -split '[\\/]')[0]
        $mean = $estimates.mean
        $rows.Add([pscustomobject]@{
            round = $round
            benchmark_id = $id
            workload = $parts[$parts.Count - 2]
            case = $parts[$parts.Count - 1]
            mean_ns = [double]$mean.point_estimate
            mean_ci_lower_ns = [double]$mean.confidence_interval.lower_bound
            mean_ci_upper_ns = [double]$mean.confidence_interval.upper_bound
        })
    }
    return $rows
}

function Write-Summary([object[]]$Cases, [string]$DerivedRoot, [string]$Mode) {
    $Cases | Sort-Object round, workload, case | Export-Csv -NoTypeInformation -Encoding utf8 (Join-Path $DerivedRoot 'cases.csv')
    $summary = New-Object System.Collections.Generic.List[object]
    foreach ($workload in @($Cases.workload | Sort-Object -Unique)) {
        foreach ($runtimeCase in @('workers-2', 'workers-4', 'workers-8')) {
            $speedups = New-Object System.Collections.Generic.List[double]
            $faster = 0
            $lowerPass = 0
            foreach ($round in @($Cases.round | Sort-Object -Unique)) {
                $inline = @($Cases | Where-Object { $_.round -eq $round -and $_.workload -eq $workload -and $_.case -eq 'inline' })
                $runtime = @($Cases | Where-Object { $_.round -eq $round -and $_.workload -eq $workload -and $_.case -eq $runtimeCase })
                if ($inline.Count -ne 1 -or $runtime.Count -ne 1) { continue }
                $speedup = $inline[0].mean_ns / $runtime[0].mean_ns
                $lower = $inline[0].mean_ci_lower_ns / $runtime[0].mean_ci_upper_ns
                $speedups.Add($speedup)
                if ($runtime[0].mean_ns -lt $inline[0].mean_ns) { $faster++ }
                if ($lower -gt 1.0) { $lowerPass++ }
            }
            $median = if ($speedups.Count -gt 0) { Get-Median ([double[]]$speedups.ToArray()) } else { [double]::NaN }
            $stable = $Mode -eq 'Formal' -and $speedups.Count -eq 5 -and $faster -ge 4 -and $median -ge 1.05 -and $lowerPass -ge 4
            $summary.Add([pscustomobject]@{
                workload = $workload
                runtime_case = $runtimeCase
                rounds = $speedups.Count
                rounds_runtime_faster = $faster
                median_speedup = $median
                rounds_conservative_lower_gt_1 = $lowerPass
                stable_win = $stable
            })
        }
    }
    $summary | Export-Csv -NoTypeInformation -Encoding utf8 (Join-Path $DerivedRoot 'crossover.csv')
}

if ($Workloads.Count -eq 0) { throw 'Workloads must contain at least one label.' }
if ($Mode -eq 'Formal' -and ($Workloads.Count -lt 3 -or $Workloads.Count -gt 4)) {
    throw 'Formal mode requires exactly 3 or 4 workload labels selected from Rapid.'
}

$outputRootFull = [System.IO.Path]::GetFullPath($OutputRoot)
$runRoot = Join-Path $outputRootFull $RunId
if (Test-Path -LiteralPath $runRoot) { throw "RunId already exists; refusing to overwrite: $runRoot" }
New-Item -ItemType Directory -Force -Path $runRoot | Out-Null

$rawRoot = Join-Path $runRoot 'raw'
$derivedRoot = Join-Path $runRoot 'derived'
$manifestRoot = Join-Path $runRoot 'manifest'
New-Item -ItemType Directory -Force -Path $rawRoot, $derivedRoot, $manifestRoot | Out-Null
$targetRoot = if ($env:CARGO_TARGET_DIR) {
    [System.IO.Path]::GetFullPath($env:CARGO_TARGET_DIR)
} else {
    [System.IO.Path]::GetFullPath((Join-Path (Get-Location) 'target'))
}
$rounds = if ($Mode -eq 'Formal') { 5 } else { 1 }
$formalOrders = @(
    @('inline', 'workers-2', 'workers-4', 'workers-8'),
    @('workers-8', 'workers-4', 'workers-2', 'inline'),
    @('workers-2', 'inline', 'workers-8', 'workers-4'),
    @('workers-4', 'workers-8', 'inline', 'workers-2'),
    @('inline', 'workers-4', 'workers-8', 'workers-2')
)
$caseOrderSeed = '0xA57C20260907CA5E'
$rapidOrder = @('inline', 'workers-1', 'workers-2', 'workers-4', 'workers-8')
$criterionArguments = if ($Mode -eq 'Formal') {
    @('--warm-up-time', '3', '--measurement-time', '5', '--sample-size', '100', '--confidence-level', '0.99', '--significance-level', '0.01', '--noplot')
} else {
    @('--warm-up-time', '0.5', '--measurement-time', '2', '--sample-size', '20', '--confidence-level', '0.99', '--significance-level', '0.01', '--noplot')
}

$oldWorkloads = $env:ASYNC_RUNTIME_CPU_WORKLOADS
$oldCaseOrder = $env:ASYNC_RUNTIME_CPU_CASE_ORDER
$oldMode = $env:ASYNC_RUNTIME_CPU_BENCH_MODE
try {
    $env:ASYNC_RUNTIME_CPU_WORKLOADS = $Workloads -join ','
    $env:ASYNC_RUNTIME_CPU_BENCH_MODE = $Mode.ToLowerInvariant()
    for ($number = 1; $number -le $rounds; $number++) {
        $roundName = 'round-{0:D2}' -f $number
        $roundRoot = Join-Path $rawRoot $roundName
        New-Item -ItemType Directory -Force -Path $roundRoot | Out-Null
        $caseOrder = if ($Mode -eq 'Formal') { $formalOrders[$number - 1] } else { $rapidOrder }
        $env:ASYNC_RUNTIME_CPU_CASE_ORDER = $caseOrder -join ','
        $roundManifest = [ordered]@{
            captured_at = (Get-Date -Format o)
            run_id = $RunId
            mode = $Mode
            round = $number
            workloads = $Workloads
            case_order = $caseOrder
            case_order_seed = $caseOrderSeed
            case_position_means = @{ inline = 2.2; workers_2 = 2.8; workers_4 = 2.4; workers_8 = 2.6 }
            command = @('cargo', 'bench', '--locked', '--bench', 'cpu_crossover', '--') + $criterionArguments
            commit = Invoke-ExternalText 'git' @('rev-parse', 'HEAD')
            working_tree = Invoke-ExternalText 'git' @('status', '--short')
            cargo_lock_sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path (Get-Location) 'Cargo.lock')).Hash
            rustc_vv = Invoke-ExternalText 'rustc' @('-vV')
            target = $env:TARGET
            os = [System.Environment]::OSVersion.VersionString
            processor = ((Get-CimInstance Win32_Processor | Select-Object -First 1 -ExpandProperty Name).Trim())
            physical_cores = (Get-CimInstance Win32_Processor | Measure-Object -Property NumberOfCores -Sum | Select-Object -ExpandProperty Sum)
            logical_processors = (Get-CimInstance Win32_Processor | Measure-Object -Property NumberOfLogicalProcessors -Sum | Select-Object -ExpandProperty Sum)
            power_scheme = Invoke-ExternalText 'powercfg' @('/getactivescheme')
            flags = @{ cargo_target_dir = $targetRoot; async_runtime_cpu_workloads = $env:ASYNC_RUNTIME_CPU_WORKLOADS; async_runtime_cpu_case_order = $env:ASYNC_RUNTIME_CPU_CASE_ORDER; async_runtime_cpu_bench_mode = $env:ASYNC_RUNTIME_CPU_BENCH_MODE }
            source_sha256 = @{
                cargo_toml = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path (Get-Location) 'Cargo.toml')).Hash
                cpu_crossover = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path (Get-Location) 'benches\cpu_crossover.rs')).Hash
                wake_benchmark = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path (Get-Location) 'benches\v030_yield_wake_storm.rs')).Hash
                runner = (Get-FileHash -Algorithm SHA256 -LiteralPath $PSCommandPath).Hash
            }
        }
        $manifestDirectory = Join-Path $manifestRoot $roundName
        New-Item -ItemType Directory -Force -Path $manifestDirectory | Out-Null
        $roundManifest | ConvertTo-Json -Depth 5 | Set-Content -Encoding utf8 (Join-Path $manifestDirectory 'environment.json')
        $log = Join-Path $roundRoot 'stdout.log'
        Write-Host "$Mode $roundName/${rounds}: $($env:ASYNC_RUNTIME_CPU_CASE_ORDER) workloads=$($env:ASYNC_RUNTIME_CPU_WORKLOADS)"
        & cargo bench --locked --bench cpu_crossover -- @criterionArguments 2>&1 | Tee-Object -FilePath $log
        if ($LASTEXITCODE -ne 0) { throw "cpu_crossover failed in $roundName (exit $LASTEXITCODE)" }
        Copy-CriterionRaw (Join-Path $targetRoot 'criterion') $roundRoot $Workloads $caseOrder
    }
    $cases = @(Read-Cases $rawRoot)
    if ($cases.Count -eq 0) { throw 'No cpu_crossover Criterion estimates were copied.' }
    Write-Summary $cases $derivedRoot $Mode
}
finally {
    $env:ASYNC_RUNTIME_CPU_WORKLOADS = $oldWorkloads
    $env:ASYNC_RUNTIME_CPU_CASE_ORDER = $oldCaseOrder
    $env:ASYNC_RUNTIME_CPU_BENCH_MODE = $oldMode
}

Write-Host "CPU crossover $Mode complete: $runRoot"
