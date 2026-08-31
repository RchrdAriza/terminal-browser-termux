#!/bin/bash
set -euo pipefail

# One-time setup for running terminal-browser natively on Termux (no X11).
# The browser is Electron (glibc) run through Termux's glibc packages plus a
# local directory of shared libraries (`~/glibc-libs`) collected because a few
# are missing from the termux-glibc repo. This script:
#   1. installs the glibc packages it needs,
#   2. ensures the missing glibc shared libraries are present,
#   3. patches the bundled electron binary so its interpreter and rpath point
#      at the glibc runtime,
#   4. prints the run command.
#
# `~/glibc-libs` may already exist from a previous setup; the script only adds
# what is missing instead of replacing it.

GLIBC_LIBS="$HOME/glibc-libs"
GLIBC_LIB="/data/data/com.termux/files/usr/glibc/lib"
GLIBC_BIN="/data/data/com.termux/files/usr/glibc/bin"
LINKER="$GLIBC_LIB/ld-linux-aarch64.so.1"
PATCHELF="$GLIBC_BIN/patchelf"

say() { printf '\033[1;36m[-]\033[0m %s\n' "$*"; }

mkdir -p "$GLIBC_LIBS"

say "installing glibc packages"
for pkg in patchelf libsqlite-glibc; do
  if [ ! -e "$GLIBC_LIB" ] || ! find "$GLIBC_LIB" "$GLIBC_BIN" -maxdepth 1 -iname "*$pkg*" -print -quit 2>/dev/null | grep -q .; then
    pkg install -y "$pkg" || true
  fi
done

say "installing glibc packages (suite)"
for pkg in libglib libstdc++ libgcc; do
  pkg install -y "$pkg" 2>/dev/null || pkg install -y "${pkg}"-glibc 2>/dev/null || true
done

say "collecting glibc shared libraries into $GLIBC_LIBS"
# Copy only real ELF shared objects, never the GNU ld scripts (libc.so, libpthread.so).
find "$GLIBC_LIB" -maxdepth 1 -type f -name '*.so*' -exec sh -c '
  for f; do
    head="$(head -c 4 "$f" 2>/dev/null)";
    if [ "$head" = "$(printf '\''\177ELF'\'')" ]; then
      cp -an "$f" "$0";
    fi;
  done
' "$GLIBC_LIBS" {} +
find "$GLIBC_LIB" -maxdepth 1 -type l -name '*.so*' \
  ! -name 'libc.so' ! -name 'libc.so.6' ! -name 'libpthread.so' \
  -exec cp -an {} "$GLIBC_LIBS/" \;
# The core runtime must come from real ELF files, not the GNU ld scripts.
for lib in libc.so.6 libm.so.6 libgcc_s.so.1 libpthread.so.0; do
  [ -e "$GLIBC_LIBS/$lib" ] || cp -an "$GLIBC_LIB/$lib" "$GLIBC_LIBS/";
done

INSTALL_DIR="${TERMINAL_BROWSER_INSTALL_DIR:-$HOME/.local/share/terminal-browser}"
ELECTRON="$INSTALL_DIR/electron/electron"

if [ -e "$ELECTRON" ]; then
  say "patching $ELECTRON"
  env -i \
    PATH="/data/data/com.termux/files/usr/bin:$GLIBC_BIN" \
    LD_LIBRARY_PATH="$GLIBC_LIBS:$GLIBC_LIB" \
    "$PATCHELF" \
      --set-interpreter "$LINKER" \
      --set-rpath "$GLIBC_LIBS:$INSTALL_DIR/electron:$GLIBC_LIB" \
      "$ELECTRON"
else
  say "electron not found at $ELECTRON (run the release installer first)"
fi

say "done. To run:"
printf '\n  TERMUX_VERSION=0.119 TERMINAL_BROWSER_BACKEND=sixel \\\n'
printf '  LD_LIBRARY_PATH=%s:%s/electron \\\n' "$GLIBC_LIBS" "$INSTALL_DIR"
printf '  %s/bin/terminal-browser open https://example.com\n\n' "$INSTALL_DIR"