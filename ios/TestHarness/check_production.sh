#!/usr/bin/env bash
# Executable production-readiness gate for the jcode iOS app.
# Evaluates every locally checkable item in ../PRODUCTION_CHECKLIST.md.
# Exit 0 = all local gates pass.
set -u
cd "$(dirname "$0")/.."   # ios/

pass=0
fail=0
check() { # check <name> <cmd...>
    local name="$1"; shift
    if "$@" >/dev/null 2>&1; then
        echo "PASS  $name"; pass=$((pass + 1))
    else
        echo "FAIL  $name"; fail=$((fail + 1))
    fi
}

# The 1024pt marketing icon is rejected if it has an alpha channel.
icon_opaque() {
    local icon
    icon=$(find Sources/JCodeMobile/Assets.xcassets/AppIcon.appiconset -name "*.png" | head -1)
    [ -n "$icon" ] && sips -g hasAlpha "$icon" | grep -q "hasAlpha: no"
}

# The demo entry must render before the pairing form: on a 4.7" iPhone
# anything after it is below the fold, and a reviewer may never scroll.
demo_above_fold() {
    python3 -c '
import sys
src = open("Sources/JCodeMobile/Views/PairingView.swift").read()
body = src.split("var body: some View", 1)[1]
demo = body.find("demoLink")
form = body.find("field(\"Host\"")
sys.exit(0 if 0 <= demo < form else 1)
'
}

plist_has() { # plist_has <key>
    /usr/libexec/PlistBuddy -c "Print :$1" Sources/JCodeMobile/Info.plist
}

echo "== code and behavior =="
check "swift test" swift test
pushd TestHarness >/dev/null
check "reward determinism" python3 -m reward.test_determinism
check "engine determinism" python3 -m reward.interaction.test_engine
popd >/dev/null

echo "== app store requirements =="
check "privacy manifest present" test -f Sources/JCodeMobile/PrivacyInfo.xcprivacy
check "privacy manifest valid plist" plutil -lint Sources/JCodeMobile/PrivacyInfo.xcprivacy
check "camera usage string" plist_has NSCameraUsageDescription
check "local network usage string" plist_has NSLocalNetworkUsageDescription
check "export compliance key" plist_has ITSAppUsesNonExemptEncryption
check "launch screen" plist_has UILaunchScreen
check "url scheme" plist_has CFBundleURLTypes:0:CFBundleURLSchemes:0
check "app icon set" test -d Sources/JCodeMobile/Assets.xcassets/AppIcon.appiconset
check "foreground reconnect handler" grep -q "scenePhase" Sources/JCodeMobile/JCodeMobileApp.swift
check "privacy manifest in app sources dir (auto-included by xcodegen)" \
    test -f Sources/JCodeMobile/PrivacyInfo.xcprivacy
check "bundle id is com.jcode.mobile" \
    grep -q "PRODUCT_BUNDLE_IDENTIFIER: com.jcode.mobile" project.yml
check "marketing version set" grep -q "MARKETING_VERSION:" project.yml
check "1024pt marketing icon has no alpha" icon_opaque
check "supports iPhone and iPad" grep -q 'TARGETED_DEVICE_FAMILY: "1,2"' project.yml

echo "== reviewer can use the app without a server =="
# Guideline 2.1: App Review has no jcode server, so a demo path that needs no
# pairing is what keeps the app from looking non-functional.
check "demo transport exists" test -f Sources/JCodeKit/DemoTransport.swift
check "demo entry point on the pairing screen" \
    grep -q "startDemo()" Sources/JCodeMobile/Views/PairingView.swift
check "demo entry is above the fold (rendered before the pairing form)" \
    demo_above_fold
check "demo mode is disclosed in the UI" \
    grep -q "Demo mode" Sources/JCodeMobile/Views/ConnectionBanner.swift
check "demo mode has an exit to pairing" \
    grep -q "exitDemo()" Sources/JCodeMobile/Views/ChatView.swift
check "app review notes checked in" test -f AppStore/REVIEW_NOTES.md
check "store metadata checked in" test -f AppStore/METADATA.md

echo "== app target compiles =="
check "xcodegen project generates" xcodegen generate
check "app target builds for simulator" xcodebuild build \
    -project JCodeMobile.xcodeproj -scheme JCodeMobile -configuration Debug \
    -destination "generic/platform=iOS Simulator" CODE_SIGNING_ALLOWED=NO

echo
echo "passed: $pass  failed: $fail"
test "$fail" -eq 0
