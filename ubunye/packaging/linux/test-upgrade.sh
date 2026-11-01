#!/usr/bin/env bash
# Ubunye-owned. Proof that the package rename upgrades cleanly: install the published Film 0.7.0
# package (named `filmcraft`), then the new one (named `ubunye-creative-film`, which replaces,
# provides and, on Debian, conflicts with `filmcraft`). Afterwards only the new package is
# installed, the app and CLI run, and the shared kopanod link points at an installed copy.
# Runs as root on a throwaway machine or container; CI only.
#
# Usage: test-upgrade.sh <old.deb|old.rpm> <new.deb|new.rpm>
set -euo pipefail
OLD="$(realpath "$1")"
NEW="$(realpath "$2")"
case "$NEW" in
  *.deb) KIND=deb ;;
  *.rpm) KIND=rpm ;;
  *) echo "error: $NEW is not a .deb or .rpm" >&2; exit 2 ;;
esac
fail() { echo "FAIL: $*" >&2; exit 1; }
step() { echo; echo "== $*"; }
name_of() { if [ $KIND = deb ]; then dpkg-deb -f "$1" Package; else rpm -qp --qf '%{NAME}' "$1"; fi; }
installed() { if [ $KIND = deb ]; then dpkg-query -W -f '${Status}' "$1" 2>/dev/null | grep -q 'install ok installed'; else rpm -q "$1" >/dev/null 2>&1; fi; }
install_pkg() { if [ $KIND = deb ]; then apt-get install -y -qq "$1" >/dev/null; else dnf install -y -q "$1" >/dev/null; fi; }

old_name="$(name_of "$OLD")"
new_name="$(name_of "$NEW")"
[ "$old_name" = filmcraft ] || fail "the old package is named $old_name, expected filmcraft"
[ "$new_name" = ubunye-creative-film ] || fail "the new package is named $new_name, expected ubunye-creative-film"

step "install the old package ($old_name)"
install_pkg "$OLD"
installed filmcraft || fail "filmcraft is not installed"
/usr/bin/filmcraft-cli --version

step "install the new package ($new_name) over it"
install_pkg "$NEW"
installed ubunye-creative-film || fail "ubunye-creative-film is not installed"
if installed filmcraft; then fail "filmcraft is still installed next to ubunye-creative-film"; fi
/usr/bin/filmcraft-cli --version | grep -F "(Ubunye Film)" || fail "filmcraft-cli does not run"
test -x /usr/bin/filmcraft || fail "/usr/bin/filmcraft is missing"
k="$(readlink -f /usr/bin/kopanod)" || fail "/usr/bin/kopanod is missing"
test -x "$k" || fail "/usr/bin/kopanod points at $k, which is not there"
echo "kopanod -> $k"

step "remove the new package"
if [ $KIND = deb ]; then apt-get remove -y -qq ubunye-creative-film >/dev/null; else dnf remove -y -q ubunye-creative-film >/dev/null; fi
if installed ubunye-creative-film; then fail "ubunye-creative-film is still installed"; fi
test ! -e /usr/bin/filmcraft || fail "/usr/bin/filmcraft is left behind"
echo PASS
