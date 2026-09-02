#!/usr/bin/env sh
set -eu
python3 scripts/verify-package-config.py
cargo build --release --locked
cargo packager --version | grep '0.11.8' >/dev/null || {
  echo 'cargo-packager 0.11.8 est requis' >&2
  exit 1
}
case "$(uname -s)" in
  Darwin) cargo packager --release --formats dmg ;;
  Linux) cargo packager --release --formats deb,appimage ;;
  *) echo 'Système non pris en charge' >&2; exit 1 ;;
esac
