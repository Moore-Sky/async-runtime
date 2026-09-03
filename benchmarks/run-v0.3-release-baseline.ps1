param(
    [ValidateRange(1, 20)]
    [int]$Rounds = 5,
    [ValidateRange(1, 1000000)]
    [int]$LatencySamples = 10000
)

$ErrorActionPreference = "Stop"
$OutputDirectory = Join-Path $PSScriptRoot "..\target\v030-release-baseline"
$OutputDirectory = [System.IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null

$Metadata = @(
    "captured_at=$(Get-Date -Format o)"
    "rounds=$Rounds"
    "latency_samples=$LatencySamples"
    "commit=$(git rev-parse HEAD)"
    "rustc=$(& rustc -V)"
    "cargo=$(& cargo -V)"
    "os=$([System.Environment]::OSVersion.VersionString)"
    "processor=$((Get-CimInstance Win32_Processor | Select-Object -First 1 -ExpandProperty Name).Trim())"
    "physical_cores=$(Get-CimInstance Win32_Processor | Measure-Object -Property NumberOfCores -Sum | Select-Object -ExpandProperty Sum)"
    "logical_processors=$(Get-CimInstance Win32_Processor | Measure-Object -Property NumberOfLogicalProcessors -Sum | Select-Object -ExpandProperty Sum)"
    "power_scheme=$(& powercfg /getactivescheme)"
    "working_tree=$(& git status --short | Out-String)"
)
$Metadata | Set-Content -Encoding utf8 (Join-Path $OutputDirectory "metadata.txt")

$Benches = @(
    @{ Name = "general_spawn"; Arguments = @() }
    @{ Name = "priority"; Arguments = @() }
    @{ Name = "v030_nested_locality"; Arguments = @() }
    @{ Name = "v030_external_producers"; Arguments = @() }
    @{ Name = "v030_completion_contention"; Arguments = @() }
    @{ Name = "v030_latency_sampling"; Arguments = @() }
    @{ Name = "v030_yield_wake_storm"; Arguments = @() }
    @{ Name = "v030_idle_wake"; Arguments = @("--features", "stats") }
    @{ Name = "v030_cpu_workload"; Arguments = @() }
    @{ Name = "v030_local_budget_latency"; Arguments = @() }
)

$env:ASYNC_RUNTIME_LATENCY_SAMPLES = "$LatencySamples"
$env:ASYNC_RUNTIME_LOCAL_BUDGET_SAMPLES = "$LatencySamples"
try {
    foreach ($Round in 1..$Rounds) {
        foreach ($Bench in $Benches) {
            $Name = $Bench.Name
            $Log = Join-Path $OutputDirectory ("round-{0:D2}-{1}.log" -f $Round, $Name)
            Write-Host "baseline round $Round/${Rounds}: $Name"
            & cargo bench --locked --bench $Name @($Bench.Arguments) 2>&1 |
                Tee-Object -FilePath $Log
            if ($LASTEXITCODE -ne 0) {
                throw "benchmark failed: round=$Round bench=$Name exit=$LASTEXITCODE"
            }
        }
    }
}
finally {
    Remove-Item Env:ASYNC_RUNTIME_LATENCY_SAMPLES -ErrorAction SilentlyContinue
    Remove-Item Env:ASYNC_RUNTIME_LOCAL_BUDGET_SAMPLES -ErrorAction SilentlyContinue
}

Write-Host "baseline complete: $OutputDirectory"
