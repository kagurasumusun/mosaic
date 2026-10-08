#!/usr/bin/env python3
"""
tools/fetch_emmc.py -- Download and combine eMMC image release parts from kagurasumusun/wince
SPDX-License-Identifier: LicenseRef-brain-emu-1.0
"""

import os
import sys
import json
import urllib.request
import urllib.parse

REPO = "kagurasumusun/wince"
TAG = "emmc-repaired4-2026-08-17"
OUTPUT = "emmc.img"

def main():
    pat = os.environ.get("PAT") or os.environ.get("GITHUB_PAT") or os.environ.get("GITHUB_TOKEN")
    if len(sys.argv) > 1 and sys.argv[1].startswith("github_pat_"):
        pat = sys.argv[1]

    if not pat:
        print("Usage: tools/fetch_emmc.py [PAT_TOKEN]")
        print("Or set PAT environment variable.")
        sys.exit(1)

    url = f"https://api.github.com/repos/{REPO}/releases/tags/{TAG}"
    req = urllib.request.Request(url, headers={"Authorization": f"token {pat}", "User-Agent": "brain-emu"})

    print(f"Fetching release metadata for {TAG} from {REPO}...")
    try:
        with urllib.request.urlopen(req) as resp:
            data = json.loads(resp.read().decode("utf-8"))
    except Exception as e:
        print(f"Failed to fetch release: {e}")
        sys.exit(1)

    assets = data.get("assets", [])
    part_assets = sorted([a for a in assets if "part-" in a["name"]], key=lambda x: x["name"])
    if not part_assets:
        print("No image part assets found in release.")
        sys.exit(1)

    part_files = []
    for asset in part_assets:
        filename = os.path.join("build", asset["name"])
        part_files.append(filename)
        if os.path.exists(filename) and os.path.getsize(filename) == asset["size"]:
            print(f"Part {asset['name']} already downloaded.")
            continue

        print(f"Downloading {asset['name']} ({asset['size']} bytes)...")
        download_url = asset["browser_download_url"]
        dl_req = urllib.request.Request(download_url, headers={"Authorization": f"token {pat}", "User-Agent": "brain-emu"})

        with urllib.request.urlopen(dl_req) as resp, open(filename, "wb") as f:
            while True:
                chunk = resp.read(1024 * 1024)
                if not chunk:
                    break
                f.write(chunk)

    print(f"Combining {len(part_files)} parts into {OUTPUT}...")
    with open(OUTPUT, "wb") as out_f:
        for pf in part_files:
            print(f" Appending {pf}...")
            with open(pf, "rb") as in_f:
                while True:
                    chunk = in_f.read(1024 * 1024)
                    if not chunk:
                        break
                    out_f.write(chunk)

    print(f"Successfully created {OUTPUT} ({os.path.getsize(OUTPUT)} bytes)!")

if __name__ == "__main__":
    main()
