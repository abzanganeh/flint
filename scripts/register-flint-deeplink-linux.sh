#!/usr/bin/env bash
# Register flint:// for local dev on Linux (xdg-mime + .desktop handler).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HANDLER="${ROOT}/scripts/flint-deeplink-handler.sh"
DESKTOP="${HOME}/.local/share/applications/flint-handler.desktop"

if [[ ! -x "${HANDLER}" ]]; then
  echo "Handler script not executable. Run: chmod +x ${HANDLER}" >&2
  exit 1
fi

# Ensure at least one debug build exists (handler will start tauri dev when needed).
if [[ ! -x "${ROOT}/src-tauri/target/debug/flint" ]]; then
  echo "Building Flint debug binary (first time only)…" >&2
  (cd "${ROOT}" && npm run tauri build -- --debug)
fi

mkdir -p "${HOME}/.local/share/applications"

cat > "${DESKTOP}" <<EOF
[Desktop Entry]
Type=Application
Name=Flint
Terminal=false
NoDisplay=true
MimeType=x-scheme-handler/flint
Exec=${HANDLER} %u
EOF

chmod +x "${HANDLER}"

update-desktop-database "${HOME}/.local/share/applications" 2>/dev/null || true
xdg-mime default flint-handler.desktop x-scheme-handler/flint

if grep -q 'target/debug/flint" %u' "${DESKTOP}" 2>/dev/null; then
  echo "ERROR: desktop file still points at raw binary — registration failed" >&2
  exit 1
fi

echo "Registered flint:// -> ${HANDLER}"
echo "Handler: ${DESKTOP}"
xdg-mime query default x-scheme-handler/flint
echo ""
echo "Dev note: flint:// cold start will launch 'npm run tauri dev' if Vite is not running."
echo "If you see a stuck error window, quit it and run: pkill -f 'target/debug/flint flint://'"
