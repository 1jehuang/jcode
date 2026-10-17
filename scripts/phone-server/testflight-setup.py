#!/usr/bin/env python3
"""Post-upload TestFlight setup for JCodeMobile.

Run after a build reaches App Store Connect (PLA must be accepted):
  /tmp/ascvenv/bin/python scripts/phone-server/testflight-setup.py

Idempotent. Does three things:
1. Finds the com.kcode.mobile app and its latest build.
2. Ensures an internal beta group exists with the account holder as tester.
3. Assigns the latest build to that group so it is installable in TestFlight.
"""

import json
import sys
import time
import urllib.request
import urllib.error

KEY_ID = "XJKP4235XC"
ISSUER = "f1147f07-48fe-4850-9171-f37d4b2dee41"
KEY_PATH = "/home/jeremy/Downloads/AuthKey_XJKP4235XC.p8"
BUNDLE_ID = "com.kcode.mobile"
TESTER_EMAIL = "jeremyhuang55555@gmail.com"
GROUP_NAME = "internal"
API = "https://api.appstoreconnect.apple.com/v1"


def token():
    import jwt

    key = open(KEY_PATH).read()
    return jwt.encode(
        {
            "iss": ISSUER,
            "iat": int(time.time()),
            "exp": int(time.time()) + 1200,
            "aud": "appstoreconnect-v1",
        },
        key,
        algorithm="ES256",
        headers={"kid": KEY_ID},
    )


def req(method, path, body=None):
    url = path if path.startswith("http") else API + path
    data = json.dumps(body).encode() if body is not None else None
    r = urllib.request.Request(url, data=data, method=method)
    r.add_header("Authorization", f"Bearer {token()}")
    if data:
        r.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(r, timeout=30) as resp:
            raw = resp.read().decode()
            return resp.status, json.loads(raw) if raw else {}
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read().decode() or "{}")


def main():
    st, apps = req("GET", f"/apps?filter[bundleId]={BUNDLE_ID}")
    if st != 200 or not apps.get("data"):
        print(f"app lookup failed ({st}): {json.dumps(apps)[:300]}")
        return 1
    app_id = apps["data"][0]["id"]
    print(f"app: {app_id}")

    st, builds = req(
        "GET",
        f"/builds?filter[app]={app_id}&sort=-uploadedDate&limit=1",
    )
    if st != 200 or not builds.get("data"):
        print(f"no builds yet ({st}): {json.dumps(builds)[:300]}")
        return 1
    build = builds["data"][0]
    build_id = build["id"]
    attrs = build["attributes"]
    print(f"latest build: {attrs.get('version')} ({attrs.get('processingState')})")

    # Beta group (create if missing)
    st, groups = req(
        "GET", f"/betaGroups?filter[app]={app_id}&filter[name]={GROUP_NAME}"
    )
    if groups.get("data"):
        group_id = groups["data"][0]["id"]
    else:
        st, g = req(
            "POST",
            "/betaGroups",
            {
                "data": {
                    "type": "betaGroups",
                    "attributes": {"name": GROUP_NAME, "isInternalGroup": True},
                    "relationships": {
                        "app": {"data": {"type": "apps", "id": app_id}}
                    },
                }
            },
        )
        if st not in (200, 201):
            print(f"group create failed ({st}): {json.dumps(g)[:300]}")
            return 1
        group_id = g["data"]["id"]
    print(f"group: {group_id}")

    # Tester (create if missing, then add to group)
    st, testers = req("GET", f"/betaTesters?filter[email]={TESTER_EMAIL}")
    if testers.get("data"):
        tester_id = testers["data"][0]["id"]
    else:
        st, t = req(
            "POST",
            "/betaTesters",
            {
                "data": {
                    "type": "betaTesters",
                    "attributes": {"email": TESTER_EMAIL, "firstName": "Jeremy"},
                    "relationships": {
                        "betaGroups": {
                            "data": [{"type": "betaGroups", "id": group_id}]
                        }
                    },
                }
            },
        )
        if st not in (200, 201):
            print(f"tester create failed ({st}): {json.dumps(t)[:300]}")
            return 1
        tester_id = t["data"]["id"]
    print(f"tester: {tester_id}")

    # Make sure tester is in the group (no-op if already)
    req(
        "POST",
        f"/betaGroups/{group_id}/relationships/betaTesters",
        {"data": [{"type": "betaTesters", "id": tester_id}]},
    )

    # Assign build to group
    st, r = req(
        "POST",
        f"/betaGroups/{group_id}/relationships/builds",
        {"data": [{"type": "builds", "id": build_id}]},
    )
    if st not in (200, 201, 204):
        print(f"build assign failed ({st}): {json.dumps(r)[:300]}")
        return 1
    print("build assigned to group. TestFlight install should be available.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
