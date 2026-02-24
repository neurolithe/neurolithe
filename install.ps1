# ──────────────────────────────────────────────────────────────
#  NeuroLithe Installer for Windows
#  One-line install: irm https://raw.githubusercontent.com/neurolithe/neurolithe/main/install.ps1 | iex
# ──────────────────────────────────────────────────────────────

$ErrorActionPreference = "Stop"

$Version = if ($env:NEUROLITHE_VERSION) { $env:NEUROLITHE_VERSION } else { "latest" }
$GitHubRepo = "neurolithe/neurolithe"
$InstallDir = if ($env:NEUROLITHE_INSTALL_DIR) { $env:NEUROLITHE_INSTALL_DIR } else { "$env:USERPROFILE\.neurolithe" }
$BinDir = "$InstallDir\bin"
$DataDir = "$InstallDir\data"
$Target = "x86_64-pc-windows-msvc"

# ── Banner ──────────────────────────────────────────────────
function Show-Banner {
    Write-Host ""
    Write-Host "  ╔══════════════════════════════════════════════╗" -ForegroundColor Cyan
    Write-Host "  ║           NeuroLithe Installer               ║" -ForegroundColor Cyan
    Write-Host "  ║   AI Memory That Thinks Like You Do          ║" -ForegroundColor Cyan
    Write-Host "  ╚══════════════════════════════════════════════╝" -ForegroundColor Cyan
    Write-Host ""
}

function Write-Info    { param($msg) Write-Host "[INFO]    $msg" -ForegroundColor Blue }
function Write-Success { param($msg) Write-Host "[OK]      $msg" -ForegroundColor Green }
function Write-Warn    { param($msg) Write-Host "[WARN]    $msg" -ForegroundColor Yellow }
function Write-Err     { param($msg) Write-Host "[ERROR]   $msg" -ForegroundColor Red; exit 1 }

# ── Resolve version ─────────────────────────────────────────
function Get-LatestVersion {
    if ($Version -eq "latest") {
        Write-Info "Fetching latest release..."
        try {
            $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$GitHubRepo/releases/latest" -Headers @{ "User-Agent" = "NeuroLithe-Installer" }
            $script:Version = $release.tag_name
        } catch {
            Write-Err "Could not determine latest version. Set `$env:NEUROLITHE_VERSION manually."
        }
    }
    Write-Info "Installing version: $Version"
}

# ── Download and extract binary ─────────────────────────────
function Install-Binary {
    $archiveName = "neurolithe-${Target}.zip"
    $url = "https://github.com/$GitHubRepo/releases/download/${Version}/${archiveName}"

    Write-Info "Downloading from: $url"

    # Create directories
    New-Item -ItemType Directory -Path $BinDir -Force | Out-Null
    New-Item -ItemType Directory -Path $DataDir -Force | Out-Null

    $tmpDir = Join-Path $env:TEMP "neurolithe-install-$(Get-Random)"
    New-Item -ItemType Directory -Path $tmpDir -Force | Out-Null

    try {
        $archivePath = Join-Path $tmpDir $archiveName
        
        # Download
        $ProgressPreference = 'SilentlyContinue'
        Invoke-WebRequest -Uri $url -OutFile $archivePath -UseBasicParsing
        $ProgressPreference = 'Continue'

        # Extract
        $extractDir = Join-Path $tmpDir "extracted"
        Expand-Archive -Path $archivePath -DestinationPath $extractDir -Force

        # Find the binary
        $binary = Get-ChildItem -Path $extractDir -Recurse -Filter "neurolithe.exe" | Select-Object -First 1

        if (-not $binary) {
            Write-Err "Could not find neurolithe.exe in the downloaded archive."
        }

        Copy-Item -Path $binary.FullName -Destination "$BinDir\neurolithe.exe" -Force
        Write-Success "Binary installed to: $BinDir\neurolithe.exe"
    }
    finally {
        Remove-Item -Path $tmpDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}

# ── Create default configuration ────────────────────────────
function New-Config {
    $configFile = "$InstallDir\neurolithe.toml"

    if (Test-Path $configFile) {
        Write-Warn "Config already exists at $configFile - skipping."
        return
    }

    @"
[llm]
# Provider: "openai", "gemini", "anthropic", or "custom" (OpenRouter, Ollama, etc.)
provider = "custom"

# Model for fact extraction and context compression
model = "openai/gpt-4o-mini"

# Model for vector embeddings (semantic search)
embedding_model = "openai/text-embedding-3-small"

# Base URL for API calls (required for "custom" provider)
# OpenRouter:      https://openrouter.ai/api/v1
# Ollama:          http://localhost:11434/v1
# LM Studio:       http://localhost:1234/v1
base_url = "https://openrouter.ai/api/v1"

[database]
# SQLite database path
path = "neurolithe.sqlite"

# Embedding vector dimension (must match your model)
# OpenAI text-embedding-3-small: 1536
# Google text-embedding-004:     768
# Nomic nomic-embed-text:        768
vector_dimension = 1536
"@ | Set-Content -Path $configFile -Encoding UTF8

    Write-Success "Config created: $configFile"
}

# ── Prompt for API key ──────────────────────────────────────
function Set-ApiKey {
    $envFile = "$InstallDir\.env"

    if (Test-Path $envFile) {
        Write-Warn "API key file already exists at $envFile - skipping."
        return
    }

    Write-Host ""
    Write-Host "  API Key Setup" -ForegroundColor Cyan
    Write-Host "  NeuroLithe needs an API key for LLM calls."
    Write-Host "  Get one free at: https://openrouter.ai/keys" -ForegroundColor White
    Write-Host ""

    $apiKey = Read-Host "  Enter your API key (or press Enter to skip)"

    if ($apiKey) {
        "NEUROLITHE_API_KEY=$apiKey" | Set-Content -Path $envFile -Encoding UTF8
        Write-Success "API key saved to: $envFile"
    } else {
        @"
# Set your API key here:
NEUROLITHE_API_KEY=your-api-key-here
"@ | Set-Content -Path $envFile -Encoding UTF8
        Write-Warn "Skipped. Edit $envFile to add your key later."
    }
}

# ── Generate MCP config ────────────────────────────────────
function New-McpConfig {
    $mcpFile = "$InstallDir\mcp-config.json"
    $binaryPath = "$BinDir\neurolithe.exe" -replace '\\', '\\\\'

    @"
{
  "mcpServers": {
    "neurolithe": {
      "command": "$binaryPath",
      "args": [],
      "env": {
        "NEUROLITHE_API_KEY": "YOUR_API_KEY_HERE"
      }
    }
  }
}
"@ | Set-Content -Path $mcpFile -Encoding UTF8

    Write-Success "MCP config generated: $mcpFile"
}

# ── Add to PATH ────────────────────────────────────────────
function Add-ToPath {
    $currentPath = [Environment]::GetEnvironmentVariable("Path", "User")

    if ($currentPath -notlike "*$BinDir*") {
        [Environment]::SetEnvironmentVariable("Path", "$BinDir;$currentPath", "User")
        $env:Path = "$BinDir;$env:Path"
        Write-Success "Added $BinDir to user PATH (restart terminal to take effect)"
    } else {
        Write-Success "PATH already contains $BinDir"
    }
}

# ── Post-install instructions ──────────────────────────────
function Show-Instructions {
    Write-Host ""
    Write-Host "  ✓ NeuroLithe installed successfully!" -ForegroundColor Green
    Write-Host ""
    Write-Host "  Installation:" -ForegroundColor White
    Write-Host "    Binary:   $BinDir\neurolithe.exe"
    Write-Host "    Config:   $InstallDir\neurolithe.toml"
    Write-Host "    API Key:  $InstallDir\.env"
    Write-Host "    Data:     $DataDir\"
    Write-Host ""
    Write-Host "  Quick Start:" -ForegroundColor White
    Write-Host ""
    Write-Host "    1. Set your API key:" -ForegroundColor Cyan
    Write-Host "       `$env:NEUROLITHE_API_KEY = 'your-key-here'" -ForegroundColor Yellow
    Write-Host ""
    Write-Host "    2. Add to your MCP client (Claude Desktop, Cursor, etc.):" -ForegroundColor Cyan
    Write-Host "       Copy the config from: $InstallDir\mcp-config.json" -ForegroundColor White
    Write-Host ""
    Write-Host "    3. Restart your terminal, then verify:" -ForegroundColor Cyan
    Write-Host "       neurolithe --help" -ForegroundColor Yellow
    Write-Host ""
    Write-Host "  Documentation: https://docs.neurolithe.com" -ForegroundColor White
    Write-Host "  GitHub:        https://github.com/$GitHubRepo" -ForegroundColor White
    Write-Host ""
}

# ── Main ────────────────────────────────────────────────────
Show-Banner
Get-LatestVersion
Install-Binary
New-Config
Set-ApiKey
New-McpConfig
Add-ToPath
Show-Instructions
