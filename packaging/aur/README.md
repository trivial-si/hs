# Arch Linux AUR packaging

These templates package `hs` for the AUR as `hs-time-bin` (prebuilt x86-64 release archive) and `hs-time` (built from source). Both install `/usr/bin/hs`, shell completions for Bash, Zsh, and Fish, and the `LICENSE` and `NOTICE` files.

## Publish or update an AUR package

After publishing a GitHub release (such as `v0.1.2`):

1. Clone the AUR repository (first time: `ssh aur@aur.archlinux.org setup-repo hs-time-bin`):

```sh
git clone ssh://aur@aur.archlinux.org/hs-time-bin.git /tmp/aur-hs-time-bin
cp packaging/aur/hs-time-bin/PKGBUILD /tmp/aur-hs-time-bin/
cd /tmp/aur-hs-time-bin
```

2. Replace `sha256sums=('SKIP')` with the real release archive checksum and generate `.SRCINFO`:

```sh
updpkgsums
makepkg --printsrcinfo > .SRCINFO
makepkg -si
```

3. Commit and push `PKGBUILD` and `.SRCINFO` to AUR:

```sh
git add PKGBUILD .SRCINFO
git commit -m "Update hs-time-bin to 0.1.2"
git push
```

Repeat the same steps for `hs-time` if you want to maintain both source and binary AUR packages.
