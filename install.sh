#!/usr/bin/env bash
set -euo pipefail

# ──────────────────────────────────────────────────────────────
#  NeuroLithe Installer
#  One-line install: curl -fsSL https://raw.githubusercontent.com/neurolithe/neurolithe/main/install.sh | bash
# ──────────────────────────────────────────────────────────────

VERSION="${NEUROLITHE_VERSION:-latest}"
GITHUB_REPO="neurolithe/neurolithe"
INSTALL_DIR="${NEUROLITHE_INSTALL_DIR:-$HOME/.neurolithe}"
BIN_DIR="$INSTALL_DIR/bin"
CONFIG_DIR="$INSTALL_DIR"
DATA_DIR="$INSTALL_DIR/data"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
YELLOW='\033[1;33m'
BOLD='\033[1m'
NC='\033[0m' # No Color

banner() {
    echo ""
    echo -e "${CYAN}${BOLD}"
    echo "  ╔══════════════════════════════════════════════╗"
    echo "  ║           NeuroLithe Installer               ║"
    echo "  ║   AI Memory That Thinks Like You Do          ║"
    echo "  ╚══════════════════════════════════════════════╝"
    echo -e "${NC}"
}

info()    { echo -e "${BLUE}[INFO]${NC}    $1"; }
success() { echo -e "${GREEN}[OK]${NC}      $1"; }
warn()    { echo -e "${YELLOW}[WARN]${NC}    $1"; }
error()   { echo -e "${RED}[ERROR]${NC}   $1"; exit 1; }

# ── Detect OS and Architecture ──────────────────────────────
detect_platform() {
    local os arch target

    os="$(uname -s)"
    arch="$(uname -m)"

    case "$os" in
        Linux)   os="unknown-linux-gnu" ;;
        Darwin)  os="apple-darwin" ;;
        MINGW*|MSYS*|CYGWIN*) os="pc-windows-msvc" ;;
        *)       error "Unsupported operating system: $os" ;;
    esac

    case "$arch" in
        x86_64|amd64)  arch="x86_64" ;;
        aarch64|arm64) arch="aarch64" ;;
        *)             error "Unsupported architecture: $arch" ;;
    esac

    # aarch64 linux is not in our matrix yet
    if [[ "$arch" == "aarch64" && "$os" == "unknown-linux-gnu" ]]; then
        error "ARM64 Linux is not yet supported. Please build from source: cargo install neurolithe"
    fi

    TARGET="${arch}-${os}"
    info "Detected platform: ${BOLD}$TARGET${NC}"
}

# ── Resolve version (latest or specific) ────────────────────
resolve_version() {
    if [[ "$VERSION" == "latest" ]]; then
        info "Fetching latest release..."
        VERSION=$(curl -fsSL "https://api.github.com/repos/$GITHUB_REPO/releases/latest" \
            | grep '"tag_name"' | head -1 | sed 's/.*"tag_name": *"//;s/".*//')
        
        if [[ -z "$VERSION" ]]; then
            error "Could not determine latest version. Set NEUROLITHE_VERSION manually."
        fi
    fi
    info "Installing version: ${BOLD}$VERSION${NC}"
}

# ── Download and extract binary ─────────────────────────────
download_binary() {
    local url archive_name tmp_dir

    if [[ "$TARGET" == *"windows"* ]]; then
        archive_name="neurolithe-${TARGET}.zip"
    else
        archive_name="neurolithe-${TARGET}.tar.gz"
    fi

    url="https://github.com/$GITHUB_REPO/releases/download/${VERSION}/${archive_name}"

    info "Downloading from: $url"

    tmp_dir=$(mktemp -d)
    trap 'rm -rf "$tmp_dir"' EXIT

    if ! curl -fSL --progress-bar "$url" -o "$tmp_dir/$archive_name"; then
        error "Download failed. Check that version '$VERSION' exists and has a release for '$TARGET'."
    fi

    # Create install directories
    mkdir -p "$BIN_DIR" "$DATA_DIR"

    # Extract
    if [[ "$archive_name" == *.zip ]]; then
        unzip -qo "$tmp_dir/$archive_name" -d "$tmp_dir/extracted"
    else
        tar xzf "$tmp_dir/$archive_name" -C "$tmp_dir/extracted" 2>/dev/null || \
        (mkdir -p "$tmp_dir/extracted" && tar xzf "$tmp_dir/$archive_name" -C "$tmp_dir/extracted")
    fi

    # Find and install the binary
    local binary
    binary=$(find "$tmp_dir/extracted" -name "neurolithe" -o -name "neurolithe.exe" | head -1)
    
    if [[ -z "$binary" ]]; then
        # Binary might be directly in the archive
        binary=$(find "$tmp_dir" -name "neurolithe" -o -name "neurolithe.exe" | head -1)
    fi

    if [[ -z "$binary" ]]; then
        error "Could not find neurolithe binary in the downloaded archive."
    fi

    cp "$binary" "$BIN_DIR/neurolithe"
    chmod +x "$BIN_DIR/neurolithe"
    success "Binary installed to: $BIN_DIR/neurolithe"
}

# ── Create default configuration ────────────────────────────
create_config() {
    local config_file="$CONFIG_DIR/neurolithe.toml"

    if [[ -f "$config_file" ]]; then
        warn "Config already exists at $config_file — skipping."
        return
    fi

    cat > "$config_file" << 'TOML'
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
# SQLite database path (relative to working directory)
path = "neurolithe.sqlite"

# Embedding vector dimension (must match your model)
# OpenAI text-embedding-3-small: 1536
# Google text-embedding-004:     768
# Nomic nomic-embed-text:        768
vector_dimension = 1536
TOML

    success "Config created: $config_file"
}

# ── Prompt for API key ──────────────────────────────────────
setup_api_key() {
    local env_file="$CONFIG_DIR/.env"

    if [[ -f "$env_file" ]]; then
        warn "API key file already exists at $env_file — skipping."
        return
    fi

    echo ""
    echo -e "${CYAN}${BOLD}API Key Setup${NC}"
    echo -e "  NeuroLithe needs an API key for LLM calls."
    echo -e "  Get one free at: ${BOLD}https://openrouter.ai/keys${NC}"
    echo ""

    # Check if we're in an interactive terminal
    if [[ -t 0 ]]; then
        read -rp "  Enter your API key (or press Enter to skip): " api_key
    else
        api_key=""
    fi

    if [[ -n "$api_key" ]]; then
        echo "NEUROLITHE_API_KEY=$api_key" > "$env_file"
        chmod 600 "$env_file"
        success "API key saved to: $env_file"
    else
        cat > "$env_file" << 'EOF'
# Set your API key here:
NEUROLITHE_API_KEY=your-api-key-here
EOF
        chmod 600 "$env_file"
        warn "Skipped. Edit $env_file to add your key later."
    fi
}

# ── Generate MCP config snippets ────────────────────────────
generate_mcp_config() {
    local mcp_file="$CONFIG_DIR/mcp-config.json"
    local binary_path="$BIN_DIR/neurolithe"

    cat > "$mcp_file" << JSON
{
  "mcpServers": {
    "neurolithe": {
      "command": "$binary_path",
      "args": [],
      "env": {
        "NEUROLITHE_API_KEY": "YOUR_API_KEY_HERE"
      }
    }
  }
}
JSON

    success "MCP config generated: $mcp_file"
}

# ── Update shell PATH ───────────────────────────────────────
setup_path() {
    local shell_rc=""
    local path_line="export PATH=\"$BIN_DIR:\$PATH\""

    # Detect shell config file
    if [[ -n "${ZSH_VERSION:-}" ]] || [[ "$SHELL" == */zsh ]]; then
        shell_rc="$HOME/.zshrc"
    elif [[ -n "${BASH_VERSION:-}" ]] || [[ "$SHELL" == */bash ]]; then
        shell_rc="$HOME/.bashrc"
        [[ -f "$HOME/.bash_profile" ]] && shell_rc="$HOME/.bash_profile"
    elif [[ "$SHELL" == */fish ]]; then
        shell_rc="$HOME/.config/fish/config.fish"
        path_line="fish_add_path $BIN_DIR"
    fi

    if [[ -n "$shell_rc" ]]; then
        if ! grep -q "$BIN_DIR" "$shell_rc" 2>/dev/null; then
            echo "" >> "$shell_rc"
            echo "# NeuroLithe" >> "$shell_rc"
            echo "$path_line" >> "$shell_rc"
            success "Added $BIN_DIR to PATH in $shell_rc"
        else
            success "PATH already configured in $shell_rc"
        fi
    fi
}

# ── Print post-install instructions ─────────────────────────
print_instructions() {
    echo ""
    echo -e "${GREEN}${BOLD}  ✓ NeuroLithe installed successfully!${NC}"
    echo ""
    echo -e "  ${BOLD}Installation:${NC}"
    echo -e "    Binary:   $BIN_DIR/neurolithe"
    echo -e "    Config:   $CONFIG_DIR/neurolithe.toml"
    echo -e "    API Key:  $CONFIG_DIR/.env"
    echo -e "    Data:     $DATA_DIR/"
    echo ""
    echo -e "  ${BOLD}Quick Start:${NC}"
    echo ""
    echo -e "    ${CYAN}1.${NC} Set your API key:"
    echo -e "       ${YELLOW}export NEUROLITHE_API_KEY=\"your-key-here\"${NC}"
    echo ""
    echo -e "    ${CYAN}2.${NC} Add to your MCP client (Claude Desktop, Cursor, etc.):"
    echo -e "       Copy the config from: ${BOLD}$CONFIG_DIR/mcp-config.json${NC}"
    echo ""
    echo -e "       Or add this to your MCP settings:"
    echo ""
    echo -e "       ${YELLOW}\"neurolithe\": {"
    echo -e "         \"command\": \"$BIN_DIR/neurolithe\","
    echo -e "         \"args\": []"
    echo -e "       }${NC}"
    echo ""
    echo -e "    ${CYAN}3.${NC} Restart your terminal, then verify:"
    echo -e "       ${YELLOW}neurolithe --help${NC}"
    echo ""
    echo -e "  ${BOLD}Documentation:${NC} https://docs.neurolithe.com"
    echo -e "  ${BOLD}GitHub:${NC}        https://github.com/$GITHUB_REPO"
    echo ""
}

# ── Main ────────────────────────────────────────────────────
main() {
    banner
    detect_platform
    resolve_version
    download_binary
    create_config
    setup_api_key
    generate_mcp_config
    setup_path
    print_instructions
}

main "$@"
