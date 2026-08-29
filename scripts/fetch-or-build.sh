#!/bin/sh
# fetch-or-build.sh — l'étape [[build]] que herdr exécute après avoir cloné le
# plugin.
#
# Chemin rapide : télécharger le binaire précompilé qui correspond à la version
# déclarée par ce checkout et à la plateforme de cette machine, vérifier son
# SHA-256, et l'installer à target/release/herdr-scratchpad — le chemin que
# visent le lanceur et l'entrée [[panes]] du manifeste.
#
# Repli : sur N'IMPORTE QUEL manque (pas de release pour cette version, pas de
# précompilé pour cette plateforme, échec de téléchargement ou de somme, un
# binaire qui refuse de tourner ici), construire depuis les sources avec cargo
# — ce que cette étape faisait inconditionnellement jusqu'ici. L'installation
# ne devient jamais plus difficile qu'avant ; elle cesse seulement d'exiger un
# toolchain Rust quand une release correspondante existe.
#
# La forme « télécharger et vérifier » est reprise de herdr-palette, elle-même
# reprise de herdr-file-viewer (Saeed Marzban, MIT).
#
# L'appariement se fait par VERSION déclarée, pas par commit : un checkout en
# avance sur le dernier tag installe quand même le binaire de ce tag au lieu
# d'imposer une compilation. L'intégrité n'en souffre pas — l'asset reste
# vérifié en SHA-256, et une version sans release publiée part en 404 droit
# vers la construction depuis les sources.
#
# SCRATCHPAD_REPO_ROOT / SCRATCHPAD_CARGO_TOML / SCRATCHPAD_OUT /
# SCRATCHPAD_BASE_URL sont surchargeables pour pouvoir exercer chaque chemin
# du script contre des fichiers locaux, sans réseau.
set -u

repo="vjeantet/herdr-scratchpad"

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
repo_root="${SCRATCHPAD_REPO_ROOT:-$script_dir/..}"
cargo_toml="${SCRATCHPAD_CARGO_TOML:-$repo_root/Cargo.toml}"
out="${SCRATCHPAD_OUT:-$repo_root/target/release/herdr-scratchpad}"
base_url="${SCRATCHPAD_BASE_URL:-https://github.com/$repo/releases/download}"

have() { command -v "$1" >/dev/null 2>&1; }

# Construire depuis les sources — le comportement d'origine, inconditionnel.
# ~/.cargo/env est sourcé parce que herdr a pu être lancé depuis le Dock, où le
# processus hérite du PATH de launchd et non de celui du shell de login ; la
# garde `[ -f ]` évite qu'un fichier absent n'avorte la construction.
build_from_source() {
  # shellcheck source=/dev/null
  [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
  if ! have cargo; then
    echo "herdr-scratchpad needs a Rust toolchain to build, but cargo was not found. Install it from https://rustup.rs then re-run: herdr plugin install $repo" >&2
    exit 1
  fi
  exec cargo build --release
}

fallback() {
  echo "herdr-scratchpad: $1 - building from source instead." >&2
  [ -n "${tmpdir:-}" ] && rm -rf "$tmpdir"
  build_from_source
}

download() { # download <url> <dest>
  if have curl; then
    curl -fsSL -o "$2" "$1"
  elif have wget; then
    wget -q -O "$2" "$1"
  else
    return 127
  fi
}

sha256_of() { # imprime l'empreinte hexadécimale du fichier $1
  if have sha256sum; then
    sha256sum "$1" | awk '{print $1}'
  elif have shasum; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    return 127
  fi
}

# --- résoudre le triplet cible depuis la plateforme -------------------------
# Toutes les cibles Linux sont en musl statique, délibérément : une
# construction glibc faite sur le runner CI refuserait de démarrer sur une
# distribution plus ancienne (Debian 12 embarque glibc 2.36) — précisément la
# machine où ceci sert le plus.
os=$(uname -s 2>/dev/null || echo unknown)
arch=$(uname -m 2>/dev/null || echo unknown)
triple=""
case "$os" in
  Darwin)
    case "$arch" in
      arm64|aarch64) triple="aarch64-apple-darwin" ;;
      x86_64|amd64)  triple="x86_64-apple-darwin" ;;
    esac
    ;;
  Linux)
    case "$arch" in
      x86_64|amd64)  triple="x86_64-unknown-linux-musl" ;;
      # Un noyau 64 bits sur un userland 32 bits — une installation Raspberry
      # Pi OS armhf d'origine sur du matériel récent — annonce ici aarch64 ou
      # armv8l alors que tous les binaires du système sont en 32 bits. getconf
      # tranche ; le test d'exécution plus bas rattrape ce que getconf rate.
      aarch64|arm64)
        if [ "$(getconf LONG_BIT 2>/dev/null || echo 64)" = "32" ]; then
          triple="armv7-unknown-linux-musleabihf"
        else
          triple="aarch64-unknown-linux-musl"
        fi
        ;;
      armv7l|armv8l) triple="armv7-unknown-linux-musleabihf" ;;
    esac
    ;;
esac
[ -n "$triple" ] || fallback "no prebuilt binary for $os/$arch"

# --- lire la version que ce checkout déclare --------------------------------
version=$(grep -E '^version *= *"' "$cargo_toml" 2>/dev/null | head -n 1 | sed -E 's/^version *= *"([^"]+)".*/\1/')
[ -n "$version" ] || fallback "could not read version from $cargo_toml"

asset="herdr-scratchpad-$triple"

tmpdir=$(mktemp -d 2>/dev/null) || fallback "could not create a temp dir"
trap 'rm -rf "$tmpdir"' EXIT

# Pour la transparence seulement, jamais un échec : quand on est dans un arbre
# git et que la release publie un marqueur COMMIT, signaler que le checkout
# porte des sources absentes du binaire publié. Un marqueur manquant n'est pas
# une erreur.
ahead_note=""
if have git && git -C "$repo_root" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  head_rev=$(git -C "$repo_root" rev-parse HEAD 2>/dev/null || echo nohead)
  if download "$base_url/v$version/COMMIT" "$tmpdir/COMMIT" 2>/dev/null; then
    release_commit=$(tr -d '[:space:]' < "$tmpdir/COMMIT" 2>/dev/null)
    if [ -n "$release_commit" ] && [ "$head_rev" != "$release_commit" ]; then
      ahead_note=" Note: this checkout ($head_rev) is ahead of the v$version release commit ($release_commit), so unreleased source is not in this binary."
    fi
  fi
fi

tmpbin="$tmpdir/$asset"
tmpsum="$tmpdir/$asset.sha256"

download "$base_url/v$version/$asset" "$tmpbin" || fallback "no prebuilt binary published for v$version ($asset)"
download "$base_url/v$version/$asset.sha256" "$tmpsum" || fallback "no checksum published for v$version ($asset.sha256)"

# Un fichier de somme par asset, contenant l'unique ligne qu'émet sha256sum :
# le job de release n'a donc jamais à rassembler les résultats de la matrice
# dans un fichier commun. Le NOM est vérifié en plus de l'empreinte : c'est ce
# qui fait qu'une somme servie pour le mauvais asset est un manque et non un
# faux positif. Le séparateur est deux espaces en mode texte de coreutils et
# ` *` en mode binaire ; accepter les deux plutôt que forcer une compilation
# pour ce détail.
expected=$(grep -E "^[0-9a-f]{64} [ *]$asset\$" "$tmpsum" 2>/dev/null | awk '{print $1}' | head -n 1)
[ -n "$expected" ] || fallback "no checksum listed for $asset"

actual=$(sha256_of "$tmpbin") || fallback "no sha-256 tool (sha256sum/shasum) available"
if [ "$actual" != "$expected" ]; then
  fallback "checksum mismatch for $asset (expected $expected, got $actual)"
fi

chmod +x "$tmpbin"

# Dernière barrière avant l'installation : l'exécuter une fois. Surtout PAS
# sans argument — ce binaire a deux vies et, sans argument, c'est la TUI
# (src/main.rs) : elle prendrait l'écran de l'installation ou s'y bloquerait.
# `--focused-pane` lit stdin jusqu'à EOF et imprime la décision ; sur une
# entrée vide, `launch::focused_pane` rend une chaîne vide et sort en 0, sans
# socket, sans disque, sans terminal. Ce qu'on prouve ici, c'est seulement que
# le noyau accepte le binaire. 126/127 sont les codes « n'a pas pu exécuter »
# du shell, ce à quoi ressemble un mauvais triplet quand uname ou getconf ont
# induit la table ci-dessus en erreur.
"$tmpbin" --focused-pane </dev/null >/dev/null 2>&1
rc=$?
if [ "$rc" -eq 126 ] || [ "$rc" -eq 127 ]; then
  fallback "the prebuilt for $triple does not run on this machine (exit $rc)"
fi

mkdir -p "$(dirname "$out")"
mv -f "$tmpbin" "$out" || fallback "could not install the verified binary to $out"
echo "herdr-scratchpad: installed prebuilt v$version ($triple), verified SHA-256.$ahead_note"
exit 0
