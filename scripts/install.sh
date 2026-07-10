#!/bin/sh
# Install the `gecko` binary from the latest GitHub Release.
#
# This script ONLY fetches and installs the small (tens-of-MB) `gecko` binary
# built by .github/workflows/release.yml (`--release --features
# real-embedder`). It never downloads the ~1.3GB embedding model or TypeDB —
# those are runtime assets. After this script finishes, run:
#   gecko up && gecko sync           # records-only, no model needed
#   gecko model fetch                # opt-in: stage the embedding model to
#                                     # enable semantic retrieval (~1.3GB)
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/k4otix/gecko/main/scripts/install.sh | sh
# or, from a checkout:
#   sh scripts/install.sh [install_dir]
#
# Env overrides:
#   GECKO_INSTALL_DIR   destination directory (default: ~/.local/bin, falls
#                       back to prompting for /usr/local/bin if not writable)
#   GECKO_VERSION       release tag to install (default: latest)
#   GECKO_REPO          GitHub "owner/repo" to fetch from (default: k4otix/gecko)
set -eu

repo="${GECKO_REPO:-k4otix/gecko}"
version="${GECKO_VERSION:-latest}"

os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
    Darwin)
        case "$arch" in
            arm64) target="aarch64-apple-darwin" ;;
            x86_64) target="x86_64-apple-darwin" ;;
            *)
                echo "install.sh: unsupported macOS arch: $arch" >&2
                exit 1
                ;;
        esac
        ;;
    Linux)
        case "$arch" in
            x86_64) target="x86_64-unknown-linux-gnu" ;;
            *)
                echo "install.sh: unsupported Linux arch: $arch (only x86_64-unknown-linux-gnu is released)" >&2
                exit 1
                ;;
        esac
        ;;
    *)
        echo "install.sh: unsupported OS: $os (releases cover macOS and Linux only)" >&2
        exit 1
        ;;
esac

# Determine the install directory (arg 1 > env override > default).
install_dir="${1:-${GECKO_INSTALL_DIR:-}}"
if [ -z "$install_dir" ]; then
    install_dir="$HOME/.local/bin"
    if [ ! -d "$install_dir" ] && ! mkdir -p "$install_dir" 2>/dev/null; then
        install_dir="/usr/local/bin"
        echo "install.sh: \$HOME/.local/bin unavailable; falling back to $install_dir (may need sudo)" >&2
    fi
fi
mkdir -p "$install_dir" 2>/dev/null || true
if [ ! -d "$install_dir" ]; then
    echo "install.sh: install directory $install_dir does not exist and could not be created" >&2
    exit 1
fi

work_dir="$(mktemp -d)"
trap 'rm -rf "$work_dir"' EXIT

fetch() {
    # $1 = url, $2 = output path
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$1" -o "$2"
    elif command -v wget >/dev/null 2>&1; then
        wget -q "$1" -O "$2"
    else
        echo "install.sh: need curl or wget to download the release asset" >&2
        exit 1
    fi
}

# Resolve the concrete tag for "latest" so the printed asset name is exact.
if [ "$version" = "latest" ]; then
    if command -v gh >/dev/null 2>&1; then
        version="$(gh release list --repo "$repo" --limit 1 --json tagName --jq '.[0].tagName' 2>/dev/null || true)"
    fi
    if [ -z "$version" ]; then
        api_url="https://api.github.com/repos/$repo/releases/latest"
        tag_file="$work_dir/tag"
        fetch "$api_url" "$tag_file"
        version="$(grep -m1 '"tag_name"' "$tag_file" | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/')"
    fi
    if [ -z "$version" ]; then
        echo "install.sh: could not resolve the latest release tag for $repo" >&2
        exit 1
    fi
fi

# macOS assets are .zip, Linux assets are .tar.gz (see release.yml).
case "$os" in
    Darwin) ext="zip" ;;
    *) ext="tar.gz" ;;
esac

asset="gecko-${version}-${target}.${ext}"
download_url="https://github.com/$repo/releases/download/$version/$asset"
archive_path="$work_dir/$asset"

echo "install.sh: downloading $download_url"
if command -v gh >/dev/null 2>&1; then
    gh release download "$version" --repo "$repo" --pattern "$asset" --dir "$work_dir" \
        || fetch "$download_url" "$archive_path"
else
    fetch "$download_url" "$archive_path"
fi

extract_dir="$work_dir/extracted"
mkdir -p "$extract_dir"
case "$ext" in
    zip) unzip -q "$archive_path" -d "$extract_dir" ;;
    tar.gz) tar -xzf "$archive_path" -C "$extract_dir" ;;
esac

bin_path="$(find "$extract_dir" -type f -name gecko | head -n1)"
if [ -z "$bin_path" ]; then
    echo "install.sh: could not find a 'gecko' binary inside $asset" >&2
    exit 1
fi

dest="$install_dir/gecko"
if cp "$bin_path" "$dest" 2>/dev/null; then
    chmod +x "$dest"
else
    fallback_dir="$HOME/.local/bin"
    if [ "$install_dir" = "$fallback_dir" ]; then
        echo "install.sh: no write access to $install_dir (already the user-writable fallback) — aborting" >&2
        exit 1
    fi

    reply="n"
    if [ -t 0 ] && [ -t 1 ]; then
        # Interactive session: ask before touching anything with sudo.
        echo "install.sh: no write access to $install_dir" >&2
        printf 'install.sh: use sudo to install gecko there? [y/N] ' >&2
        read -r reply || reply="n"
    else
        # Non-interactive (e.g. `curl | sh`): never invoke sudo here — it
        # would either hang waiting for a password with no TTY attached, or
        # silently elevate privileges with no one able to confirm. Fall
        # back to a user-writable directory instead.
        echo "install.sh: no write access to $install_dir; non-interactive session, skipping sudo" >&2
    fi

    case "$reply" in
        [Yy]*)
            echo "install.sh: installing to $install_dir with sudo" >&2
            sudo cp "$bin_path" "$dest"
            sudo chmod +x "$dest"
            ;;
        *)
            install_dir="$fallback_dir"
            mkdir -p "$install_dir"
            dest="$install_dir/gecko"
            echo "install.sh: installing to $install_dir instead (user-writable, no sudo needed)" >&2
            cp "$bin_path" "$dest"
            chmod +x "$dest"
            ;;
    esac
fi

echo "install.sh: installed gecko $version ($target) to $dest"
case ":$PATH:" in
    *":$install_dir:"*) ;;
    *) echo "install.sh: NOTE — $install_dir is not on your PATH; add it to your shell profile" ;;
esac

cat <<'EOF'

Next steps:
  1. gecko up && gecko sync   # start the pinned TypeDB and sync a bundle
                              # (records-only — no embedding model needed)

Want semantic retrieval too? That's an explicit opt-in:
  2. gecko model fetch       # one-time download of the embedding model (~1.3GB)
  3. Set [semantic_index] enabled = true and embedder = "bge-large-en-v1.5"
     in gecko.toml

See docs/INSTALL.md for the air-gapped / enterprise path (staged model +
external TypeDB, no runtime downloads).
EOF
