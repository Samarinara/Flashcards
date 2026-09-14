#!/bin/sh
set -eu

project_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
manifest="$project_root/build-aux/page.polli.Flashcards.json"
build_dir="${FLATPAK_BUILD_DIR:-$project_root/.flatpak-build}"
state_dir="${FLATPAK_STATE_DIR:-$project_root/.flatpak-builder}"
output_dir="${FLATPAK_OUTPUT_DIR:-$project_root/dist}"
repo_dir="$output_dir/repo"

version=$(
  sed -n 's/^version = "\([^"]*\)"/\1/p' "$project_root/Cargo.toml" |
    head -n 1
)
arch=$(flatpak --default-arch)
bundle="$output_dir/Flashcards-$version-$arch.flatpak"

if [ -z "$version" ]; then
  echo "Could not determine the package version from Cargo.toml." >&2
  exit 1
fi

if [ ! -f "$project_root/build-aux/cargo-sources.json" ]; then
  echo "build-aux/cargo-sources.json is missing." >&2
  echo "See docs/distribution.md for regeneration instructions." >&2
  exit 1
fi

mkdir -p "$output_dir"

runtime_remote="${FLATPAK_RUNTIME_REMOTE:-}"
if [ -z "$runtime_remote" ]; then
  runtime_remote=$(
    flatpak info --show-origin org.gnome.Sdk//50 2>/dev/null || true
  )
fi

sdk_location=$(
  flatpak info --show-location org.gnome.Sdk//50 2>/dev/null || true
)
case "$sdk_location" in
  /var/lib/flatpak/*)
    install_scope=--system
    ;;
  *)
    install_scope=--user
    ;;
esac

if [ -n "$runtime_remote" ]; then
  flatpak-builder \
    --force-clean \
    "$install_scope" \
    --install-deps-from="$runtime_remote" \
    --state-dir="$state_dir" \
    --repo="$repo_dir" \
    "$build_dir" \
    "$manifest"
else
  flatpak-builder \
    --force-clean \
    --state-dir="$state_dir" \
    --repo="$repo_dir" \
    "$build_dir" \
    "$manifest"
fi

flatpak build-update-repo --generate-static-deltas "$repo_dir"

runtime_repo_url="${FLATPAK_RUNTIME_REPO_URL:-}"
if [ -n "$runtime_repo_url" ]; then
  flatpak build-bundle \
    "$repo_dir" \
    "$bundle" \
    page.polli.Flashcards \
    stable \
    --runtime-repo="$runtime_repo_url"
else
  flatpak build-bundle \
    "$repo_dir" \
    "$bundle" \
    page.polli.Flashcards \
    stable
fi

bundle_name=$(basename "$bundle")
(
  cd "$output_dir"
  sha256sum "$bundle_name" >"$bundle_name.sha256"
)

echo "Flatpak bundle: $bundle"
echo "SHA-256 checksum: $bundle.sha256"
echo "Flatpak repository: $repo_dir"
