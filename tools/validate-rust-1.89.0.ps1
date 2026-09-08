$ErrorActionPreference = 'Stop'

$contractName = 'PROMPTFORGE_RUST_1_89_0_BIN'
$requiredVersion = '1.89.0'
$contractBin = [Environment]::GetEnvironmentVariable($contractName)

if ([string]::IsNullOrWhiteSpace($contractBin)) {
    throw "$contractName must be set to an absolute Rust $requiredVersion bin directory"
}

$contractBin = $contractBin.Trim()
if (-not [IO.Path]::IsPathRooted($contractBin)) {
    throw "$contractName must be an absolute directory: '$contractBin'"
}
if (-not (Test-Path -LiteralPath $contractBin -PathType Container)) {
    throw "$contractName directory does not exist: '$contractBin'"
}
$contractBin = (Resolve-Path -LiteralPath $contractBin).Path

$tools = @{}
foreach ($name in @('cargo', 'rustc')) {
    $path = Join-Path $contractBin "$name.exe"
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "$contractName must contain regular cargo.exe and rustc.exe files: '$contractBin'"
    }
    $item = Get-Item -LiteralPath $path
    if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "$contractName must contain regular cargo.exe and rustc.exe files: '$contractBin'"
    }
    $tools[$name] = $item.FullName
}

function Read-RustVersion {
    param(
        [Parameter(Mandatory = $true)][string] $Name,
        [Parameter(Mandatory = $true)][string] $Path
    )

    $lines = @(& $Path '--version' 2>&1)
    $exitCode = $LASTEXITCODE
    $text = ($lines | Out-String).Trim()
    if ($exitCode -ne 0) {
        throw "$Name.exe failed at '$Path' with exit code ${exitCode}: $text"
    }
    $match = [regex]::Match(
        $text,
        "^$([regex]::Escape($Name))\s+(\d+\.\d+\.\d+)(?:\s|$)"
    )
    if (-not $match.Success) {
        throw "$Name.exe returned malformed version output at '$Path': $text"
    }
    $actualVersion = $match.Groups[1].Value
    if ($actualVersion -ne $requiredVersion) {
        throw "$Name.exe reports $actualVersion, but $contractName requires exactly $requiredVersion"
    }
    Write-Host "Validated $Name $actualVersion from $Path"
}

Read-RustVersion -Name 'cargo' -Path $tools['cargo']
Read-RustVersion -Name 'rustc' -Path $tools['rustc']

function Test-RustupProxy {
    param(
        [Parameter(Mandatory = $true)][string] $Name,
        [Parameter(Mandatory = $true)][string] $Path
    )

    $output = @(& $Path "+$requiredVersion" '--version' 2>&1)
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        return $false
    }
    $text = ($output | Out-String).Trim()
    return [regex]::IsMatch(
        $text,
        "^$([regex]::Escape($Name))\s+$([regex]::Escape($requiredVersion))(?:\s|$)"
    )
}

$cargoIsProxy = Test-RustupProxy -Name 'cargo' -Path $tools['cargo']
$rustcIsProxy = Test-RustupProxy -Name 'rustc' -Path $tools['rustc']
if ($cargoIsProxy -ne $rustcIsProxy) {
    throw "$contractName must not mix direct Rust tools and rustup proxies"
}

if (-not [string]::IsNullOrWhiteSpace($env:GITHUB_PATH)) {
    $contractBin | Add-Content -LiteralPath $env:GITHUB_PATH
}

exit 0
