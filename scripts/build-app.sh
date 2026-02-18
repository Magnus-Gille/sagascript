#!/bin/bash
# Build FlowDictate as a proper macOS app bundle
set -e

APP_NAME="FlowDictate"
BUILD_DIR=".build/release"
APP_BUNDLE="$BUILD_DIR/$APP_NAME.app"

echo "Building $APP_NAME..."

# Stamp git hash into BuildInfo.swift
GIT_HASH=$(git rev-parse --short HEAD)
BUILD_NUM=$(git rev-list --count HEAD)
BUILDINFO_FILE="Sources/FlowDictate/BuildInfo.swift"

cp "$BUILDINFO_FILE" "/tmp/BuildInfo.swift.bak"
sed -i '' "s/static let gitHash = \"dev\"/static let gitHash = \"$GIT_HASH\"/" "$BUILDINFO_FILE"
sed -i '' "s/static let build = \"1\"/static let build = \"$BUILD_NUM\"/" "$BUILDINFO_FILE"

# Build release
swift build -c release

# Restore BuildInfo.swift so git stays clean
mv "/tmp/BuildInfo.swift.bak" "$BUILDINFO_FILE"

# Create app bundle structure
rm -rf "$APP_BUNDLE"
mkdir -p "$APP_BUNDLE/Contents/MacOS"
mkdir -p "$APP_BUNDLE/Contents/Resources"

# Copy executable
cp "$BUILD_DIR/$APP_NAME" "$APP_BUNDLE/Contents/MacOS/"

# Copy Info.plist from AppBundle directory
cp "AppBundle/Info.plist" "$APP_BUNDLE/Contents/"

# Copy any resources (excluding .gitkeep)
if [ -d "Sources/FlowDictate/Resources" ]; then
    find "Sources/FlowDictate/Resources" -type f ! -name ".gitkeep" -exec cp {} "$APP_BUNDLE/Contents/Resources/" \;
fi

# Ad-hoc code sign the app (required for Input Monitoring permission to work)
echo "Signing app bundle..."
codesign --force --deep --sign - "$APP_BUNDLE"

echo "✓ Built and signed $APP_BUNDLE"
echo "Run with: open $APP_BUNDLE"
