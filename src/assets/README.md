# Flint brand assets

| File | Product | Deployed to |
|------|---------|-------------|
| `flint-logo-resume.png` | **Flint Resume** (web) — CV script + up arrow wordmark | `smart-resume/frontend/public/brand/logo.png` |
| `flint-logo-extension.png` | **Flint extension** — CV monogram icon (square) | `flint-extension/icons/icon{16,32,48,128}.png` |
| `flint-logo-desktop.png` | **Flint desktop** — CV + up arrow inside circle | `flint-logo.png` (app default) |

Palette: navy `#243447`, gold gradient `#E8944A` → `#F4B942`.

To refresh extension toolbar icons after editing `flint-logo-extension.png`:

```bash
cd flint-extension
python3 ../Flint/scripts/resize-extension-icons.py   # or re-run asset sync from repo root
npm run build
```
