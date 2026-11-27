#!/usr/bin/env python3
"""Update version and SHA-256 checksums in Homebrew and AUR packaging templates.

Usage:
    python3 scripts/update-packaging-checksums.py v0.1.2 /path/to/SHA256SUMS
"""

from pathlib import Path
import re
import sys


def parse_checksums(path: Path) -> dict[str, str]:
    sums: dict[str, str] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line:
            continue
        digest, filename = line.split(None, 1)
        filename = filename.lstrip("*")
        sums[filename] = digest
    return sums


def main() -> None:
    if len(sys.argv) != 3:
        raise SystemExit(__doc__)

    tag = sys.argv[1].strip()
    version = tag[1:] if tag.startswith("v") else tag
    sums = parse_checksums(Path(sys.argv[2]))

    root = Path(__file__).resolve().parents[1]
    targets = {
        "macos-aarch64": f"hs-v{version}-macos-aarch64.tar.gz",
        "macos-x86_64": f"hs-v{version}-macos-x86_64.tar.gz",
        "linux-aarch64": f"hs-v{version}-linux-aarch64.tar.gz",
        "linux-x86_64": f"hs-v{version}-linux-x86_64.tar.gz",
    }
    missing = [name for name in targets.values() if name not in sums]
    if missing:
        raise SystemExit("Missing expected archives in SHA256SUMS:\n" + "\n".join(missing))

    formula_path = root / "packaging" / "homebrew" / "hs.rb"
    formula = formula_path.read_text(encoding="utf-8")
    formula = re.sub(r'^  version ".*"$', f'  version "{version}"', formula, flags=re.MULTILINE)
    for key, filename in targets.items():
        digest = sums[filename]
        pattern = rf'(hs-v#{{version}}-{re.escape(key)}\.tar\.gz"\n\s+sha256 ")[^"]+(")'
        formula, count = re.subn(pattern, rf"\g<1>{digest}\g<2>", formula)
        if count != 1:
            raise SystemExit(f"Could not update Homebrew sha256 for {key}")
    formula_path.write_text(formula, encoding="utf-8")

    aur_bin_path = root / "packaging" / "aur" / "hs-time-bin" / "PKGBUILD"
    aur_bin = aur_bin_path.read_text(encoding="utf-8")
    aur_bin = re.sub(r"^pkgver=.*$", f"pkgver={version}", aur_bin, flags=re.MULTILINE)
    aur_bin = re.sub(
        r"^sha256sums=\(.*\)$",
        f"sha256sums=('{sums[targets['linux-x86_64']]}')",
        aur_bin,
        flags=re.MULTILINE,
    )
    aur_bin_path.write_text(aur_bin, encoding="utf-8")

    aur_src_path = root / "packaging" / "aur" / "hs-time" / "PKGBUILD"
    aur_src = aur_src_path.read_text(encoding="utf-8")
    aur_src = re.sub(r"^pkgver=.*$", f"pkgver={version}", aur_src, flags=re.MULTILINE)
    aur_src_path.write_text(aur_src, encoding="utf-8")

    print(f"Updated Homebrew and AUR packaging templates for v{version}.")


if __name__ == "__main__":
    main()
