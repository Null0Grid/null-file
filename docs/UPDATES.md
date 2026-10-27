# How Null File updates

| Installed from | How it updates |
|---|---|
| `.deb` | The package adds `/etc/apt/sources.list.d/null-file.sources` and the Null Grid signing key. New versions come with `sudo apt update && sudo apt upgrade`, KDE Discover or GNOME Software. |
| `.rpm` | The package adds `/etc/yum.repos.d/null-file.repo` (and `/etc/zypp/repos.d/` for openSUSE) plus the key in `/etc/pki/rpm-gpg/`. New versions come with `dnf upgrade` / `zypper up` / your software center. |
| AppImage, `.tar.gz` | Null File checks GitHub once a day. **Update now** downloads the new file, checks it against the release's `SHA256SUMS`, replaces itself and restarts. |
| Flatpak, Snap, AUR | Updated by `flatpak update`, `snap refresh`, or your AUR helper. Null File shows a notice when a new version is out. |

Settings → Updates turns the daily check off. The check is one request to `api.github.com/repos/Null0Grid/null-file/releases/latest`; nothing about you or your files is sent.

## The package repository

Every release, the **Release** workflow's `repo` job builds a signed APT + RPM repository with `packaging/repo/publish-repo.sh` and pushes it to the `gh-pages` branch. GitHub Pages serves it at **https://null0grid.github.io/null-file/** (landing page with manual setup steps).

- APT: `apt/dists/stable/InRelease` (clear-signed), `main/binary-amd64|arm64`.
- RPM: `rpm/x86_64|aarch64/`, each package signed, `repodata/repomd.xml.asc` signed.
- Signing key: **Null Grid Packages**, RSA 4096, fingerprint `F479E27ABDCF2FFF9E8AB0F47AE17832BAEAD2EC`. The public key is `packaging/repo/null-grid.asc` (and the binary keyring `null-grid-archive-keyring.gpg`).

### One-time setup (repo owner)

1. The repo has to be **public** (release downloads and Pages need it).
2. **Settings → Secrets and variables → Actions → New repository secret**: name `REPO_SIGNING_KEY`, value = the whole armored private key (`-----BEGIN PGP PRIVATE KEY BLOCK-----` …). Without it the release still works, but the `repo` job skips publishing with a warning.
3. After the first release with the secret: **Settings → Pages → Build and deployment → Deploy from a branch → `gh-pages` / root**.

Keep an offline backup of the private key. If it's ever lost or leaked: make a new key, replace the two public files in `packaging/repo/`, update the secret, and release. Installs pick up the new key with that release's package; anyone who skipped it re-adds the key from the landing page.

## Testing an update locally

`NULL_FILE_UPDATE_URL` points the update check at any `releases/latest`-style JSON, for example a local server with a test release.
