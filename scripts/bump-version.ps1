param(
    [Parameter(Mandatory = $true)]
    [string]$Version
)

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent (Split-Path -Parent $PSCommandPath)

if ($Version -notmatch '^\d+\.\d+\.\d+$') {
    Write-Error "Invalid semver format: '$Version'. Expected X.Y.Z"
    exit 1
}

$tauriConf = Join-Path $root 'apps/desktop/src-tauri/tauri.conf.json'
$packageJson = Join-Path $root 'apps/desktop/package.json'
$cargoToml = Join-Path $root 'apps/desktop/src-tauri/Cargo.toml'

# 1. Update tauri.conf.json
Write-Host "Updating $tauriConf ..."
$json = Get-Content $tauriConf -Raw | ConvertFrom-Json
$json.version = $Version
$json | ConvertTo-Json -Depth 32 | Set-Content $tauriConf -Encoding UTF8

# 2. Update package.json
Write-Host "Updating $packageJson ..."
$pkg = Get-Content $packageJson -Raw | ConvertFrom-Json
$pkg.version = $Version
$pkg | ConvertTo-Json -Depth 32 | Set-Content $packageJson -Encoding UTF8

# 3. Update Cargo.toml (regex replace first version = line in [package])
Write-Host "Updating $cargoToml ..."
$content = Get-Content $cargoToml -Raw
$content = $content -replace '(?m)^(version\s*=\s*")[\d\.]+(")', "`${1}${Version}`${2}"
Set-Content $cargoToml -Value $content -Encoding UTF8 -NoNewline

# 4. Verify build
Write-Host "`nRunning cargo check ..."
Push-Location $root
try {
    cargo check -p ask-myself-desktop --no-default-features
    if ($LASTEXITCODE -ne 0) {
        Write-Error "cargo check failed"
        exit 1
    }
} finally {
    Pop-Location
}

# 5. Git commit + tag
Write-Host "`nCreating git commit and tag ..."
Push-Location $root
try {
    git add $tauriConf $packageJson $cargoToml
    git commit -m "chore: bump version to $Version"
    git tag "v$Version"
} finally {
    Pop-Location
}

Write-Host "`n✅ Version bumped to $Version"
Write-Host "Run to publish:"
Write-Host "  git push && git push --tags"
