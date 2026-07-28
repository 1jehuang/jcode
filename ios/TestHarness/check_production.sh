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

echo
echo "passed: $pass  failed: $fail"
test "$fail" -eq 0
