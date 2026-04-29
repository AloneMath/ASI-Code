param(
    [string]$ReportDir = ".\artifacts",
    [string]$OutFile = "",
    [switch]$Append,
    [switch]$PassThru
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Escape-MarkdownCell([string]$Text) {
    if ($null -eq $Text) { return "-" }
    if ([string]::IsNullOrWhiteSpace($Text)) { return "-" }
    return ($Text -replace "\|", "\|")
}

function Infer-CategoryFromFileName([string]$FileName) {
    $name = if ($null -eq $FileName) { "" } else { $FileName.Trim().ToLowerInvariant() }
    if ($name.StartsWith("smoke_hook_matrix")) { return "hook_matrix_smoke_failure" }
    if ($name.StartsWith("smoke_hooks_cli_advanced")) { return "hooks_cli_advanced_smoke_failure" }
    if ($name.StartsWith("smoke_gateway")) { return "gateway_smoke_failure" }
    if ($name.StartsWith("smoke_review_json")) { return "review_json_smoke_failure" }
    if ($name.StartsWith("smoke_api_compat_min")) { return "api_compat_smoke_failure" }
    if ($name.StartsWith("smoke_api_compat")) { return "api_compat_smoke_failure" }
    if ($name.StartsWith("smoke_provider_model")) { return "provider_model_smoke_failure" }
    if ($name.StartsWith("smoke_tokenizer_timeout")) { return "tokenizer_smoke_failure" }
    if ($name.StartsWith("smoke_checkpoint")) { return "checkpoint_smoke_failure" }
    return ""
}

function Infer-HintFromCategory([string]$Category) {
    $c = if ($null -eq $Category) { "" } else { $Category.Trim().ToLowerInvariant() }
    switch ($c) {
        "binary_missing" { return "Build release binary first: cargo build --release." }
        "config_missing_credentials" { return "Set provider credentials (ApiKey/BaseUrl) for the selected smoke script." }
        "api_compat_smoke_failure" { return "Run smoke_api_compat.ps1 or smoke_api_compat_min.ps1 standalone and verify provider/model access." }
        "provider_model_smoke_failure" { return "Run smoke_provider_model.ps1 standalone and inspect provider/model fallback behavior." }
        "review_json_smoke_failure" { return "Run smoke_review_json.ps1 standalone; inspect review JSON schema/stats fields." }
        "hook_matrix_smoke_failure" { return "Run smoke_hook_matrix.ps1 standalone and verify ASI_HOOK_CONFIG_PATH and strict permission hook behavior." }
        "hooks_cli_advanced_smoke_failure" { return "Run smoke_hooks_cli_advanced.ps1 standalone; verify edit-handler none semantics and validate --strict pass/fail cases." }
        "tokenizer_smoke_failure" { return "Run smoke_tokenizer_timeout.ps1 standalone and verify repo path/timeouts." }
        "checkpoint_smoke_failure" { return "Run smoke_checkpoint.ps1 standalone and verify sessions/checkpoint write access." }
        "gateway_smoke_failure" { return "Run smoke_gateway.ps1 standalone; inspect provider auth/network/model diagnostics." }
        default { return "" }
    }
}

function Resolve-CategoryHint([object]$Obj, [string]$FileName) {
    $props = @{}
    if ($null -ne $Obj.PSObject -and $null -ne $Obj.PSObject.Properties) {
        foreach ($p in $Obj.PSObject.Properties) {
            $props[$p.Name] = $p.Value
        }
    }

    $status = ""
    $category = ""
    $hint = ""
    if ($props.ContainsKey("status") -and $null -ne $props["status"]) {
        $status = [string]$props["status"]
    }
    if ($props.ContainsKey("failure_category") -and $null -ne $props["failure_category"]) {
        $category = [string]$props["failure_category"]
    }
    if ($props.ContainsKey("hint") -and $null -ne $props["hint"]) {
        $hint = [string]$props["hint"]
    }

    if ($status.Trim().ToLowerInvariant() -eq "fail") {
        if ([string]::IsNullOrWhiteSpace($category)) {
            $category = Infer-CategoryFromFileName $FileName
        }
        if ([string]::IsNullOrWhiteSpace($hint)) {
            $hint = Infer-HintFromCategory $category
        }
    }

    return @{
        category = $category
        hint = $hint
    }
}

function Resolve-DurationText([object]$Obj) {
    $props = @{}
    if ($null -ne $Obj.PSObject -and $null -ne $Obj.PSObject.Properties) {
        foreach ($p in $Obj.PSObject.Properties) {
            $props[$p.Name] = $p.Value
        }
    }

    if ($props.ContainsKey("duration_secs") -and $null -ne $props["duration_secs"]) {
        return [string]$props["duration_secs"]
    }

    if ($props.ContainsKey("metrics") -and $null -ne $props["metrics"]) {
        $m = $props["metrics"]
        if ($null -ne $m.PSObject -and $null -ne $m.PSObject.Properties) {
            foreach ($mp in $m.PSObject.Properties) {
                if ($mp.Name -eq "duration_secs" -and $null -ne $mp.Value) {
                    return [string]$mp.Value
                }
            }
        }
    }

    return ""
}

$lines = New-Object System.Collections.Generic.List[string]

if (-not (Test-Path -LiteralPath $ReportDir)) {
    $null = $lines.Add("No smoke report directory found: $ReportDir")
} else {
    $files = @(Get-ChildItem -Path $ReportDir -Filter *.json -File | Sort-Object Name)
    if ($files.Count -eq 0) {
        $null = $lines.Add("No smoke report JSON files were produced.")
    } else {
        $files = @($files | Where-Object { $_.Name -like "smoke_*.json" })
        if ($files.Count -eq 0) {
            $null = $lines.Add("No smoke report JSON files matching smoke_*.json were produced.")
            $content = ($lines -join "`n")
            if (-not [string]::IsNullOrWhiteSpace($OutFile)) {
                $dir = Split-Path -Parent $OutFile
                if (-not [string]::IsNullOrWhiteSpace($dir) -and -not (Test-Path -LiteralPath $dir)) {
                    New-Item -ItemType Directory -Force -Path $dir | Out-Null
                }
                if ($Append) {
                    $content | Out-File -FilePath $OutFile -Encoding utf8 -Append
                } else {
                    $content | Out-File -FilePath $OutFile -Encoding utf8
                }
            }
            if ($PassThru -or [string]::IsNullOrWhiteSpace($OutFile)) {
                Write-Output $content
            }
            return
        }
        $null = $lines.Add("## Smoke Report Summary")
        $null = $lines.Add("")
        $null = $lines.Add("| file | status | failure_category | hint | duration_secs |")
        $null = $lines.Add("|---|---|---|---|---:|")
        foreach ($file in $files) {
            try {
                $raw = Get-Content -LiteralPath $file.FullName -Raw
                $obj = $raw | ConvertFrom-Json
                $status = Escape-MarkdownCell ([string]$obj.status)
                $resolved = Resolve-CategoryHint -Obj $obj -FileName $file.Name
                $category = Escape-MarkdownCell ([string]$resolved.category)
                $hint = Escape-MarkdownCell ([string]$resolved.hint)
                $duration = Escape-MarkdownCell (Resolve-DurationText -Obj $obj)
                $null = $lines.Add("| $($file.Name) | $status | $category | $hint | $duration |")
            } catch {
                $msg = Escape-MarkdownCell $_.Exception.Message
                $null = $lines.Add("| $($file.Name) | parse_error | - | $msg | - |")
            }
        }
    }
}

$content = ($lines -join "`n")

if (-not [string]::IsNullOrWhiteSpace($OutFile)) {
    $dir = Split-Path -Parent $OutFile
    if (-not [string]::IsNullOrWhiteSpace($dir) -and -not (Test-Path -LiteralPath $dir)) {
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
    }
    if ($Append) {
        $content | Out-File -FilePath $OutFile -Encoding utf8 -Append
    } else {
        $content | Out-File -FilePath $OutFile -Encoding utf8
    }
}

if ($PassThru -or [string]::IsNullOrWhiteSpace($OutFile)) {
    Write-Output $content
}
