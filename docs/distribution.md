# Flatpak distribution

Flashcards is packaged as `page.polli.Flashcards` on the `stable` branch. The
release manifest builds Cargo dependencies offline and exports both a
single-file bundle and an OSTree repository.

## Build a release

Install `flatpak`, `flatpak-builder`, the GNOME 50 SDK, and the Rust SDK
extension. If the SDK is already installed, the packaging script discovers its
origin and asks Flatpak Builder to obtain any missing build extension from the
same remote.

Run:

```sh
./build-aux/package-flatpak.sh
```

The outputs are:

- `dist/Flashcards-<version>-<architecture>.flatpak`
- `dist/Flashcards-<version>-<architecture>.flatpak.sha256`
- `dist/repo/`

The app version comes from `Cargo.toml`. To embed a hint telling installers
where to obtain the GNOME runtime, set a runtime repository descriptor URL:

```sh
FLATPAK_RUNTIME_REPO_URL=https://example.org/runtime.flatpakrepo \
  ./build-aux/package-flatpak.sh
```

The runtime repository does not host Flashcards; it only supplies
`org.gnome.Platform//50`.

## Test the bundle

Install and run it:

```sh
flatpak install --user ./dist/Flashcards-0.1.0-$(flatpak --default-arch).flatpak
flatpak run page.polli.Flashcards
```

Remove the test installation with:

```sh
flatpak uninstall --user page.polli.Flashcards
```

## Publish a bundle

Upload the `.flatpak` file to an HTTPS download location or a source-hosting
release along with its `.sha256` file. Users can verify and install it with:

```sh
sha256sum --check Flashcards-0.1.0-x86_64.flatpak.sha256
flatpak install --user ./Flashcards-0.1.0-x86_64.flatpak
```

A bundle is convenient for direct downloads but does not configure automatic
app updates.

## Publish an update repository

Use the repository in `dist/repo` when automatic updates are required. Public
repositories should be GPG-signed.

Choose or create a dedicated GPG key, then sign the app commit and repository
summary:

```sh
flatpak build-sign dist/repo page.polli.Flashcards stable \
  --gpg-sign=YOUR_GPG_KEY_ID

flatpak build-update-repo dist/repo \
  --generate-static-deltas \
  --gpg-sign=YOUR_GPG_KEY_ID
```

Export the public key as a single-line base64 value:

```sh
gpg --export YOUR_GPG_KEY_ID | base64 -w0
```

Upload the complete contents of `dist/repo` without changing its directory
structure. Replace the placeholders in
`build-aux/page.polli.Flashcards.flatpakrepo.in`:

- `@REPOSITORY_URL@` is the HTTPS URL of the uploaded repository directory.
- `@BASE64_GPG_PUBLIC_KEY@` is the exported public key.

Publish the resulting file as, for example, `polli.flatpakrepo`. Users add it
and install Flashcards with:

```sh
flatpak remote-add --if-not-exists --user polli \
  https://polli.page/flatpak/polli.flatpakrepo
flatpak install --user polli page.polli.Flashcards
```

Later releases are built with the same application ID, branch, repository, and
signing key. Once the updated `dist/repo` is uploaded, users receive it through:

```sh
flatpak update
```

The `.flatpakref.in` template can provide a one-click app-specific installer.
Fill its repository URL, GPG key, and the descriptor URL for the repository
that supplies the GNOME runtime.

## Refresh Cargo sources

`build-aux/cargo-sources.json` pins every crate in `Cargo.lock`. Regenerate it
whenever the lockfile changes:

```sh
git clone https://github.com/flatpak/flatpak-builder-tools.git /tmp/flatpak-builder-tools
python3 /tmp/flatpak-builder-tools/cargo/flatpak-cargo-generator.py \
  Cargo.lock -o build-aux/cargo-sources.json
```

The generator requires Python 3 with `aiohttp` and `tomlkit`.
