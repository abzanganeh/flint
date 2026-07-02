# Installer signing runbook (manual gate)

Flint release builds are wired in `.github/workflows/release.yml`. The workflow
runs on `workflow_dispatch` or version tags (`v*`). Signing is **optional** until
you add the secrets below — unsigned artifacts still upload for smoke validation.

Actual signed/notarized installers are a **manual gate** (requires paid Apple and
Windows certificates this repo cannot provision automatically).

## Trigger a release build

1. Tag a version: `git tag v0.1.0 && git push origin v0.1.0`
2. Or use **Actions → Release → Run workflow** (optional tag input).

Draft GitHub releases are created; download artifacts from the run or release page.

## macOS — Developer ID + notarization

### Accounts and certs to create

1. [Apple Developer Program](https://developer.apple.com/programs/) membership ($99/yr).
2. **Certificates, Identifiers & Profiles → Certificates → +**
   - Create **Developer ID Application** (distribution outside Mac App Store).
3. Export the cert + private key as `.p12` from Keychain Access (set an export password).
4. Create an [app-specific password](https://appleid.apple.com/account/manage) for notarization API.

### GitHub repository secrets

| Secret | Value |
| --- | --- |
| `APPLE_CERTIFICATE` | Base64 of `.p12`: `base64 -i cert.p12 \| pbcopy` |
| `APPLE_CERTIFICATE_PASSWORD` | Export password for the `.p12` |
| `APPLE_SIGNING_IDENTITY` | e.g. `Developer ID Application: Your Name (TEAMID)` |
| `APPLE_ID` | Apple ID email used for notarization |
| `APPLE_PASSWORD` | App-specific password (not your Apple ID password) |
| `APPLE_TEAM_ID` | 10-character Team ID from Apple Developer portal |

Settings → Secrets and variables → Actions → **New repository secret**.

### Verify locally (optional)

```bash
# After importing cert to login keychain:
codesign --verify --deep --strict /path/to/Flint.app
xcrun notarytool submit Flint.dmg --apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID" --wait
```

## Windows — Authenticode

### Cert to obtain

Purchase an **Extended Validation (EV)** or standard **Code Signing** certificate
from a trusted CA (DigiCert, Sectigo, etc.). Export as password-protected `.pfx`.

### GitHub repository secrets

| Secret | Value |
| --- | --- |
| `WINDOWS_CERTIFICATE` | Base64 of `.pfx`: `certutil -encode cert.pfx base64.txt` (Windows) |
| `WINDOWS_CERTIFICATE_PASSWORD` | `.pfx` export password |

The release workflow imports the cert into `Cert:\CurrentUser\My` before `tauri build`.

### Verify locally (optional)

```powershell
Get-AuthenticodeSignature .\Flint_0.1.0_x64-setup.exe
```

Status should be **Valid**; SmartScreen reputation builds over time after first signed release.

## Linux

AppImage/deb builds in CI are **unsigned** (expected). Distribute via checksums +
GitHub release notes. GPG signing of packages is out of v1 scope.

## Manual gate checklist

Before marking installer signing closed in release docs:

- [ ] All six Apple secrets set; macOS `.dmg` downloads without Gatekeeper block
- [ ] `spctl -a -vv -t install Flint.app` reports accepted/notarized on a clean Mac
- [ ] Windows secrets set; installer shows Valid Authenticode signature
- [ ] Release workflow green on all three platform matrix legs for the tag

Until then, leave **installer signing** in `manual_gate_backlog`.
