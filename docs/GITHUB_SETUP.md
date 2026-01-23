# GitHub Setup

This document describes how to push the project to GitHub and set up CI/CD.

## Prerequisites

1. GitHub CLI installed: `brew install gh`
2. Authenticated: `gh auth login`

## One-Command Setup

If GitHub CLI is authenticated, run:

```bash
# Create a new private repository and push
gh repo create flowdictate --private --source=. --push
```

Or for a public repository:

```bash
gh repo create flowdictate --public --source=. --push
```

## Manual Setup

If you prefer manual setup or the above fails:

1. **Create repository on GitHub:**
   - Go to https://github.com/new
   - Name: `flowdictate`
   - Choose public or private
   - Don't initialize with README (we have one)

2. **Add remote and push:**
   ```bash
   git remote add origin https://github.com/YOUR_USERNAME/flowdictate.git
   git push -u origin main
   ```

## After Push

Once pushed, the following will be automatically set up:

### GitHub Actions CI
- Builds on every push and PR
- Runs on macOS 14 (Sonoma)
- Runs `swift build` and `swift test`
- Located in `.github/workflows/ci.yml`

### Dependabot
- Monitors Swift Package Manager dependencies
- Monitors GitHub Actions versions
- Creates PRs weekly for updates
- Located in `.github/dependabot.yml`

## Code Signing (For Distribution)

To distribute the app, you'll need:

1. **Apple Developer Account** ($99/year)
2. **Developer ID Certificate** for distribution outside App Store
3. **Notarization** for Gatekeeper approval

### Steps:

1. Generate Developer ID certificate in Apple Developer portal
2. Download and install in Keychain
3. Add signing identity to Xcode or use:
   ```bash
   codesign --sign "Developer ID Application: Your Name" --options runtime FlowDictate.app
   ```
4. Notarize the app:
   ```bash
   xcrun notarytool submit FlowDictate.zip --apple-id YOUR_APPLE_ID --team-id YOUR_TEAM_ID --password APP_SPECIFIC_PASSWORD --wait
   xcrun stapler staple FlowDictate.app
   ```

### For Unsigned Apps

If you don't have a Developer account, users can allow the unsigned app:

1. Right-click the app and select "Open"
2. Click "Open" in the security dialog
3. Or: System Settings > Privacy & Security > "Open Anyway"

## Releases

To create a release:

```bash
# Tag the version
git tag v1.0.0
git push origin v1.0.0

# Create GitHub release
gh release create v1.0.0 --generate-notes
```

## Secrets

If you add features requiring secrets (e.g., auto-update server):

1. Go to repository Settings > Secrets and variables > Actions
2. Add repository secrets
3. Reference in workflows as `${{ secrets.SECRET_NAME }}`

Never commit secrets to the repository.
