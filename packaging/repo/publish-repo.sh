#!/usr/bin/env bash
# Build the signed Null File package repository (APT + DNF/zypper) from a
# folder of release packages.
#
#   packaging/repo/publish-repo.sh <dir with .deb and .rpm files> <out dir> [base url]
#
# Signs with the key whose fingerprint is in $REPO_KEY_ID, already imported
# into gpg (the release workflow imports the REPO_SIGNING_KEY secret).
#
# Output layout (served by GitHub Pages from the gh-pages branch):
#   apt/dists/stable/{InRelease,Release,Release.gpg}
#   apt/dists/stable/main/binary-{amd64,arm64}/Packages{,.gz}
#   apt/pool/main/n/null-file/*.deb
#   rpm/{x86_64,aarch64}/*.rpm + repodata/ (repomd.xml signed: repomd.xml.asc)
#   null-grid.asc, index.html
#
# Needs: apt-utils (apt-ftparchive), createrepo-c, rpm (rpmsign), gpg.
set -euo pipefail
src="$(cd "$1" && pwd)"
out="$2"
base="${3:-https://null0grid.github.io/null-file}"
here="$(cd "$(dirname "$0")" && pwd)"
: "${REPO_KEY_ID:?set REPO_KEY_ID to the signing key fingerprint}"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
rm -rf "$out/apt" "$out/rpm"

# ---- APT ----
pool="$out/apt/pool/main/n/null-file"
mkdir -p "$pool"
cp "$src"/null-file_*.deb "$pool/"
cd "$out/apt"
for arch in amd64 arm64; do
  d="dists/stable/main/binary-$arch"
  mkdir -p "$d"
  apt-ftparchive --arch "$arch" packages pool > "$d/Packages"
  gzip -9nkf "$d/Packages"
done
apt-ftparchive \
  -o APT::FTPArchive::Release::Origin="Null Grid" \
  -o APT::FTPArchive::Release::Label="Null Grid" \
  -o APT::FTPArchive::Release::Suite="stable" \
  -o APT::FTPArchive::Release::Codename="stable" \
  -o APT::FTPArchive::Release::Architectures="amd64 arm64" \
  -o APT::FTPArchive::Release::Components="main" \
  -o APT::FTPArchive::Release::Description="Null Grid apps (nullgrid.dev)" \
  release dists/stable > dists/stable/Release
gpg --batch --yes --local-user "$REPO_KEY_ID" --clearsign -o dists/stable/InRelease dists/stable/Release
gpg --batch --yes --local-user "$REPO_KEY_ID" -abs -o dists/stable/Release.gpg dists/stable/Release

# ---- RPM (dnf and zypper) ----
for arch in x86_64 aarch64; do
  d="$out/rpm/$arch"
  mkdir -p "$d"
  cp "$src"/null-file-*."$arch".rpm "$d/" 2>/dev/null || true
  for r in "$d"/*.rpm; do
    [ -e "$r" ] || continue
    rpmsign --define "_gpg_name $REPO_KEY_ID" --define "__gpg /usr/bin/gpg" --addsign "$r" >/dev/null
  done
  createrepo_c --quiet --update "$d"
  gpg --batch --yes --local-user "$REPO_KEY_ID" --detach-sign --armor -o "$d/repodata/repomd.xml.asc" "$d/repodata/repomd.xml"
done

# ---- key and a landing page ----
cp "$here/null-grid.asc" "$out/null-grid.asc"
cp "$here/null-grid-archive-keyring.gpg" "$out/null-grid-archive-keyring.gpg"
cp "$here/null-file.sources" "$out/null-file.sources"
cp "$here/null-file.repo" "$out/null-file.repo"
touch "$out/.nojekyll"
v="$(ls "$pool" | sed -n 's/^null-file_\([^_]*\)_.*/\1/p' | sort -V | tail -1)"
cat > "$out/index.html" <<EOF
<!doctype html><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Null File packages</title>
<style>body{background:#0a0a0f;color:#d9def0;font:15px/1.6 system-ui,sans-serif;max-width:760px;margin:40px auto;padding:0 16px}
h1{font:600 20px ui-monospace,monospace;letter-spacing:.08em}h1 b{color:#00e5ff}code,pre{font-family:ui-monospace,monospace;background:#10121a;border:1px solid #232842;border-radius:4px}
pre{padding:12px;overflow:auto}code{padding:1px 5px}a{color:#00e5ff}</style>
<h1>NULL <b>FILE</b> · package repository</h1>
<p>Latest: <b>$v</b>. Installing a Null File <code>.deb</code> or <code>.rpm</code> from the
<a href="https://github.com/Null0Grid/null-file/releases">releases</a> adds this repository for you, so updates
come with your other system updates. To add it by hand:</p>
<h2>Debian, Ubuntu, Mint, Null OS</h2>
<pre>sudo curl -fsSLo /usr/share/keyrings/null-grid-archive-keyring.gpg $base/null-grid-archive-keyring.gpg
sudo curl -fsSLo /etc/apt/sources.list.d/null-file.sources $base/null-file.sources
sudo apt update &amp;&amp; sudo apt install null-file</pre>
<h2>Fedora, RHEL-family with WebKitGTK 4.1, openSUSE</h2>
<pre>sudo curl -fsSLo /etc/pki/rpm-gpg/RPM-GPG-KEY-null-grid $base/null-grid.asc
sudo rpm --import /etc/pki/rpm-gpg/RPM-GPG-KEY-null-grid
sudo curl -fsSLo /etc/yum.repos.d/null-file.repo $base/null-file.repo     # openSUSE: /etc/zypp/repos.d/
sudo dnf install null-file</pre>
<p>Signing key: <code>$REPO_KEY_ID</code></p>
EOF
echo "Repository written to $out (null-file $v)"
