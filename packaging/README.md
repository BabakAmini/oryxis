# Packaging

Package manager manifests for distributing Oryxis. These are versioned here;
publishing to each registry is a separate manual step (see below).

When cutting a new release, bump the version and `sha256`/`hash` fields in
each manifest to match the new release artifacts.

## AUR (Arch Linux) — `aur/`

`oryxis-bin` installs the prebuilt `oryxis-linux-<arch>.tar.gz` release
artifact. The AUR is its own git server (one repo per package), reached
over SSH at `aur.archlinux.org`.

**Publishing is automated.** The `aur` job in
`.github/workflows/release.yml` runs on every `v*` tag after the GitHub
release is created: it stamps `pkgver` and the `sha256sums` into the
`PKGBUILD` template kept here, regenerates `.SRCINFO` with `makepkg`
inside an Arch container, and pushes to the AUR
(KSXGitHub/github-actions-deploy-aur). The in-repo `PKGBUILD` therefore
carries the *last manually stamped* version; CI overrides the volatile
fields at release time.

One-time setup for the automation:

1. Generate a dedicated key: `ssh-keygen -t ed25519 -f aur -C aur@ci`
2. Add the **public** key at https://aur.archlinux.org/ under
   My Account -> SSH Public Key.
3. Add the **private** key as the `AUR_SSH_PRIVATE_KEY` repo secret.

Manual fallback (run from a checkout of the AUR repo, not this repo):

```bash
git clone ssh://aur@aur.archlinux.org/oryxis-bin.git
cp /path/to/oryxis/packaging/aur/{PKGBUILD,.SRCINFO} oryxis-bin/
cd oryxis-bin
git add PKGBUILD .SRCINFO
git commit -m "Update to X.Y.Z"
git push
```

For a manual push, bump `pkgver` / `sha256sums` in both files first; on an
Arch machine `.SRCINFO` is regenerated with
`makepkg --printsrcinfo > .SRCINFO`.

## Scoop (Windows) — `scoop/`

`oryxis.json` installs the `oryxis-windows-<arch>.zip` release artifact.
Two ways to ship it:

- **Personal bucket (no review):** add an `oryxis.json` to any git repo with a
  `bucket/` folder. Users install with:

  ```
  scoop bucket add oryxis https://github.com/wilsonglasser/oryxis
  scoop install oryxis
  ```

  (Requires the manifest to live under a `bucket/` directory in the bucket
  repo. This repo keeps it under `packaging/scoop/` for versioning; copy it
  into a `bucket/` folder of whichever repo serves as the bucket.)

- **Official `extras` bucket (discoverable):** open a PR to
  `ScoopInstaller/Extras` adding `bucket/oryxis.json`. Users then install with
  `scoop install extras/oryxis` without adding a custom bucket. This is the
  better path for discovery.

## Flathub (Linux) — `flatpak/`

App ID `app.oryxis.Oryxis` (3-segment rDNS under the oryxis.app domain). The
manifest builds from a pinned post-v0.8.0 commit (the one that adds the
metainfo, desktop file and Wayland app_id fix, all absent from the v0.8.0 tag);
the desktop, metainfo and icons come from that checkout. Only two files ship to
the Flathub repo: the manifest and `cargo-sources.json`.

Regenerate `cargo-sources.json` whenever `Cargo.lock` changes:

```bash
pip install aiohttp tomlkit
python flatpak-cargo-generator.py Cargo.lock -o packaging/flatpak/cargo-sources.json
```

(`flatpak-cargo-generator.py` lives in flatpak/flatpak-builder-tools.)

Publish:

1. Fork `flathub/flathub`, branch `app.oryxis.Oryxis`.
2. Copy `app.oryxis.Oryxis.yml` and `cargo-sources.json` to the repo root.
3. Open a PR against the `master` branch. The Flathub buildbot compiles the
   app in the sandbox; iterate until it goes green, then a reviewer merges and
   `flathub/app.oryxis.Oryxis` is created.
4. After publishing, claim the "Verified" badge from the oryxis.app domain
   (DNS TXT or `.well-known`).

Local test (needs flatpak + flatpak-builder):

```bash
flatpak install flathub org.freedesktop.Sdk//24.08 org.freedesktop.Platform//24.08 \
  org.freedesktop.Sdk.Extension.rust-stable//24.08
flatpak-builder --user --install --force-clean build-dir packaging/flatpak/app.oryxis.Oryxis.yml
```
