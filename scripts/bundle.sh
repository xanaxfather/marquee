#!/bin/sh
# Build Marquee.app (menu-bar only, no Dock icon) into ./dist
set -eu
cd "$(dirname "$0")/.."
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
cargo build --release
APP="dist/Marquee.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/marquee "$APP/Contents/MacOS/marquee"
cp assets/Marquee.icns "$APP/Contents/Resources/Marquee.icns"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Marquee</string>
    <key>CFBundleDisplayName</key><string>Marquee</string>
    <key>CFBundleIdentifier</key><string>io.github.xanaxfather.marquee</string>
    <key>CFBundleExecutable</key><string>marquee</string>
    <key>CFBundleIconFile</key><string>Marquee</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>${VERSION}</string>
    <key>CFBundleVersion</key><string>${VERSION}</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>LSUIElement</key><true/>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
codesign --force --sign - "$APP" >/dev/null 2>&1 || echo "warning: ad-hoc codesign failed (app still runs locally)"
echo "built $APP ($(du -sh "$APP/Contents/MacOS/marquee" | cut -f1))"
