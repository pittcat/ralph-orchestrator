#!/usr/bin/env pwsh
[CmdletBinding()]
param(
    [ValidateSet("Baseline", "Compile", "Loop", "Cleanup", "Docs", "Full")]
    [string]$Mode = "Baseline",

    [string]$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$script:WindowsRustTarget = "x86_64-pc-windows-msvc"
$script:SupportBoundaryFiles = @(
    "README.md",
    "docs/reference/faq.md",
    "docs/reference/troubleshooting.md"
)

if (Get-Variable -Name PSNativeCommandUseErrorActionPreference -ErrorAction SilentlyContinue) {
    $PSNativeCommandUseErrorActionPreference = $true
}

function Write-SmokeStatus {
    param(
        [string]$Level,
        [string]$Message
    )

    Write-Host "[$Level] $Message"
}

function Resolve-RepoRootPath {
    param(
        [string]$Candidate
    )

    $resolvedPath = Resolve-Path -Path $Candidate -ErrorAction Stop
    return $resolvedPath.Path
}

function Assert-CommandAvailable {
    param(
        [string]$CommandName
    )

    if (-not (Get-Command -Name $CommandName -ErrorAction SilentlyContinue)) {
        throw "Required command '$CommandName' was not found in PATH."
    }
}

function Invoke-SmokeCommand {
    param(
        [string]$Label,
        [scriptblock]$Command
    )

    Write-SmokeStatus -Level "RUN" -Message $Label

    $global:LASTEXITCODE = 0
    & $Command
    if ($LASTEXITCODE) {
        throw "Command failed with exit code ${LASTEXITCODE}: $Label"
    }

    Write-SmokeStatus -Level "PASS" -Message $Label
}

function Assert-PathPresent {
    param(
        [string]$Path
    )

    if (-not (Test-Path -Path $Path)) {
        throw "Required path '$Path' does not exist."
    }
}

function Assert-RustTargetInstalled {
    param(
        [string]$TargetTriple
    )

    Assert-CommandAvailable -CommandName "rustup"

    $global:LASTEXITCODE = 0
    $installedTargets = & rustup target list --installed
    if ($LASTEXITCODE) {
        throw "Unable to read installed Rust targets with rustup."
    }

    if ($installedTargets -notcontains $TargetTriple) {
        throw "Rust target '$TargetTriple' is not installed. Run 'rustup target add $TargetTriple' before rerunning this smoke suite."
    }
}

function Assert-AnyFileMatches {
    param(
        [string[]]$Paths,
        [string]$Pattern,
        [string]$Reason
    )

    $matchingPaths = @()
    foreach ($path in $Paths) {
        Assert-PathPresent -Path $path
        $content = Get-Content -Path $path -Raw
        if ($content -match $Pattern) {
            $matchingPaths += $path
        }
    }

    if ($matchingPaths.Count -eq 0) {
        throw "Pattern '$Pattern' was not found in any support-boundary file. $Reason"
    }
}

function Assert-NoFileMatches {
    param(
        [string[]]$Paths,
        [string]$Pattern,
        [string]$Reason
    )

    $matchingPaths = @()
    foreach ($path in $Paths) {
        Assert-PathPresent -Path $path
        $content = Get-Content -Path $path -Raw
        if ($content -match $Pattern) {
            $matchingPaths += $path
        }
    }

    if ($matchingPaths.Count -gt 0) {
        $joinedPaths = $matchingPaths -join ", "
        throw "Pattern '$Pattern' still appears in: $joinedPaths. $Reason"
    }
}

function New-SmokeCheck {
    param(
        [string]$Name,
        [string]$Summary,
        [scriptblock]$Action
    )

    return [pscustomobject]@{
        Name = $Name
        Summary = $Summary
        Action = $Action
    }
}

function New-SmokeGroup {
    param(
        [string]$Name,
        [string]$Description,
        [object[]]$Checks = @()
    )

    return [pscustomobject]@{
        Name = $Name
        Description = $Description
        Checks = @($Checks)
    }
}

function Get-SmokeSuite {
    return [ordered]@{
        compile = New-SmokeGroup `
            -Name "compile" `
            -Description "Compilation and target-level validation entry point." `
            -Checks @(
                (New-SmokeCheck `
                    -Name "windows-target-installed" `
                    -Summary "Rust toolchain includes the Windows MSVC target required by the spec." `
                    -Action {
                        Assert-RustTargetInstalled -TargetTriple $script:WindowsRustTarget
                    }
                ),
                (New-SmokeCheck `
                    -Name "workspace-check" `
                    -Summary "Workspace compiles for the Windows MSVC target." `
                    -Action {
                        Assert-CommandAvailable -CommandName "cargo"
                        Invoke-SmokeCommand `
                            -Label "cargo check --workspace --target $script:WindowsRustTarget" `
                            -Command { cargo check --workspace --target $script:WindowsRustTarget }
                    }
                )
            )
        loop = New-SmokeGroup `
            -Name "loop" `
            -Description "Primary and parallel loop behavior validation entry point." `
            -Checks @(
                (New-SmokeCheck `
                    -Name "worktree-link-strategy" `
                    -Summary "Cross-platform worktree/shared-state coverage exists in ralph-core." `
                    -Action {
                        Assert-CommandAvailable -CommandName "cargo"
                        Invoke-SmokeCommand `
                            -Label "cargo test -p ralph-core --test platform_cross_platform worktree_link_strategy" `
                            -Command { cargo test -p ralph-core --test platform_cross_platform worktree_link_strategy }
                    }
                ),
                (New-SmokeCheck `
                    -Name "run-list-stop" `
                    -Summary "Windows loop integration coverage exists for run/list/stop behavior." `
                    -Action {
                        Assert-CommandAvailable -CommandName "cargo"
                        Invoke-SmokeCommand `
                            -Label "cargo test -p ralph-cli --test integration_windows_loops run_list_stop" `
                            -Command { cargo test -p ralph-cli --test integration_windows_loops run_list_stop }
                    }
                ),
                (New-SmokeCheck `
                    -Name "web-unsupported" `
                    -Summary "The CLI has an explicit Windows-unsupported contract for 'ralph web'." `
                    -Action {
                        Assert-CommandAvailable -CommandName "cargo"
                        Invoke-SmokeCommand `
                            -Label "cargo test -p ralph-cli --test integration_windows_loops web_unsupported_on_windows" `
                            -Command { cargo test -p ralph-cli --test integration_windows_loops web_unsupported_on_windows }
                    }
                )
            )
        cleanup = New-SmokeGroup `
            -Name "cleanup" `
            -Description "Process cleanup and backend termination validation entry point." `
            -Checks @(
                (New-SmokeCheck `
                    -Name "process-control" `
                    -Summary "Cross-platform process control coverage exists in ralph-core." `
                    -Action {
                        Assert-CommandAvailable -CommandName "cargo"
                        Invoke-SmokeCommand `
                            -Label "cargo test -p ralph-core --test platform_cross_platform process_control" `
                            -Command { cargo test -p ralph-core --test platform_cross_platform process_control }
                    }
                ),
                (New-SmokeCheck `
                    -Name "stop-and-orphan-cleanup" `
                    -Summary "Windows loop cleanup coverage exists for stop/orphan cleanup behavior." `
                    -Action {
                        Assert-CommandAvailable -CommandName "cargo"
                        Invoke-SmokeCommand `
                            -Label "cargo test -p ralph-cli --test integration_windows_loops stop_and_orphan_cleanup" `
                            -Command { cargo test -p ralph-cli --test integration_windows_loops stop_and_orphan_cleanup }
                    }
                ),
                (New-SmokeCheck `
                    -Name "backend-cleanup" `
                    -Summary "Adapters expose Windows backend cleanup coverage." `
                    -Action {
                        Assert-CommandAvailable -CommandName "cargo"
                        Invoke-SmokeCommand `
                            -Label "cargo test -p ralph-adapters --test windows_backend_cleanup" `
                            -Command { cargo test -p ralph-adapters --test windows_backend_cleanup }
                    }
                )
            )
        docs = New-SmokeGroup `
            -Name "docs" `
            -Description "Documentation and support-boundary validation entry point." `
            -Checks @(
                (New-SmokeCheck `
                    -Name "powershell-completions" `
                    -Summary "PowerShell completion generation remains wired through the CLI." `
                    -Action {
                        Assert-CommandAvailable -CommandName "cargo"
                        Invoke-SmokeCommand `
                            -Label "cargo run -p ralph-cli --bin ralph -- completions powershell" `
                            -Command { cargo run -p ralph-cli --bin ralph -- completions powershell | Out-Null }
                    }
                ),
                (New-SmokeCheck `
                    -Name "support-boundary-docs" `
                    -Summary "Support docs describe native Windows support and a Windows-unsupported 'ralph web' boundary." `
                    -Action {
                        Assert-NoFileMatches `
                            -Paths $script:SupportBoundaryFiles `
                            -Pattern "Windows \(with WSL\)" `
                            -Reason "Support-boundary docs should no longer describe the core CLI as WSL-only."
                        Assert-NoFileMatches `
                            -Paths $script:SupportBoundaryFiles `
                            -Pattern "WSL-only" `
                            -Reason "Support-boundary docs should no longer describe the core CLI as WSL-only."
                        Assert-AnyFileMatches `
                            -Paths $script:SupportBoundaryFiles `
                            -Pattern "(?is)ralph web.*unsupported|unsupported.*ralph web" `
                            -Reason "One of README / FAQ / Troubleshooting should document the explicit Windows-unsupported 'ralph web' contract."
                    }
                )
            )
    }
}

function Resolve-SmokeGroups {
    param(
        [string]$RequestedMode,
        [System.Collections.IDictionary]$Suite
    )

    switch ($RequestedMode.ToLowerInvariant()) {
        "baseline" { return @("compile", "loop", "cleanup", "docs") }
        "full" { return @("compile", "loop", "cleanup", "docs") }
        default {
            $groupName = $RequestedMode.ToLowerInvariant()
            if (-not $Suite.Contains($groupName)) {
                throw "Unsupported smoke mode '$RequestedMode'."
            }
            return @($groupName)
        }
    }
}

function Invoke-SmokeCheck {
    param(
        [object]$Group,
        [object]$Check
    )

    Write-SmokeStatus -Level "CHECK" -Message "$($Group.Name): $($Check.Name) - $($Check.Summary)"
    & $Check.Action
}

function Invoke-SmokeGroup {
    param(
        [object]$Group
    )

    Write-Host ""
    Write-Host ("== {0} ==" -f $Group.Name.ToUpperInvariant())
    Write-Host $Group.Description

    if ($Group.Checks.Count -eq 0) {
        Write-SmokeStatus -Level "TODO" -Message "No checks registered for mode '$($Group.Name)' yet."
        return
    }

    foreach ($check in $Group.Checks) {
        Invoke-SmokeCheck -Group $Group -Check $check
    }
}

function Invoke-SmokeSuite {
    param(
        [string]$RequestedMode,
        [string]$ResolvedRepoRoot
    )

    $suite = Get-SmokeSuite
    $groupNames = Resolve-SmokeGroups -RequestedMode $RequestedMode -Suite $suite

    Write-SmokeStatus -Level "INFO" -Message "Repo root: $ResolvedRepoRoot"
    Write-SmokeStatus -Level "INFO" -Message "Mode: $RequestedMode"

    foreach ($groupName in $groupNames) {
        Invoke-SmokeGroup -Group $suite[$groupName]
    }
}

$resolvedRepoRoot = Resolve-RepoRootPath -Candidate $RepoRoot

Push-Location $resolvedRepoRoot
try {
    Invoke-SmokeSuite -RequestedMode $Mode -ResolvedRepoRoot $resolvedRepoRoot
}
finally {
    Pop-Location
}
