#!/usr/bin/env python3
"""Push snap/local/store-listing.yaml to the Snap Store listing.

Sets the categories, contact and website links, the banner and the
screenshots of the snap named in snap/snapcraft.yaml, using the store
credentials in $SNAPCRAFT_STORE_CREDENTIALS (`snapcraft export-login`).
Media that already match by hash are not uploaded again.

Needs: pip install craft-store pyyaml
"""

import hashlib
import json
import sys
from pathlib import Path

import craft_store
import yaml

STORE_URL = "https://dashboard.snapcraft.io"
UPLOAD_URL = "https://storage.snapcraftcontent.com"
LOGIN_URL = "https://login.ubuntu.com"
MAX_SCREENSHOTS = 5

ROOT = Path(__file__).resolve().parents[2]


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def client() -> craft_store.UbuntuOneStoreClient:
    return craft_store.UbuntuOneStoreClient(
        base_url=STORE_URL,
        storage_base_url=UPLOAD_URL,
        auth_url=LOGIN_URL,
        endpoints=craft_store.endpoints.U1_SNAP_STORE,
        application_name="colorz-store-listing",
        user_agent="colorz-store-listing",
        environment_auth="SNAPCRAFT_STORE_CREDENTIALS",
        ephemeral=True,
    )


def snap_id(store, name: str) -> str:
    account = store.request(
        "GET", f"{STORE_URL}/dev/api/account", headers={"Accept": "application/json"}
    ).json()
    try:
        return account["snaps"]["16"][name]["snap-id"]
    except KeyError:
        sys.exit(f"The snap {name!r} is not registered to this account.")


def push_metadata(store, sid: str, listing: dict) -> None:
    metadata = {
        key: listing[key] for key in ("categories", "contact", "website") if key in listing
    }
    if not metadata:
        return
    store.request(
        "POST",
        f"{STORE_URL}/dev/api/snaps/{sid}/metadata",
        json=metadata,
        headers={"Content-Type": "application/json", "Accept": "application/json"},
    )
    print("Updated:", ", ".join(metadata))


def wanted_media(listing: dict) -> list[tuple[str, Path]]:
    """(type, path) of the banner and screenshots, in display order."""
    media = []
    if "banner" in listing:
        media.append(("banner", ROOT / listing["banner"]))
    shots = listing.get("screenshots", [])
    if len(shots) > MAX_SCREENSHOTS:
        sys.exit(f"At most {MAX_SCREENSHOTS} screenshots are allowed.")
    media += [("screenshot", ROOT / s) for s in shots]
    for _, path in media:
        if not path.is_file():
            sys.exit(f"Missing file: {path}")
    return media


def push_media(store, sid: str, listing: dict) -> None:
    url = f"{STORE_URL}/dev/api/snaps/{sid}/binary-metadata"
    current = store.request("GET", url, headers={"Accept": "application/json"}).json()
    media = wanted_media(listing)

    def key(m):
        return (m["type"], m.get("hash"))

    managed = {"banner", "screenshot"}
    kept = [m for m in current if m.get("type") not in managed]
    have = [key(m) for m in current if m.get("type") in managed]
    want = [(kind, sha256(path)) for kind, path in media]
    if have == want:
        print("Banner and screenshots are up to date.")
        return

    info, files = list(kept), {}
    for i, (kind, path) in enumerate(media):
        name = f"{kind}-{i}"
        info.append(
            {"key": name, "type": kind, "filename": path.name, "hash": sha256(path)}
        )
        files[name] = (path.name, path.read_bytes(), "image/png")
    # PUT replaces the whole set, so media no longer listed are removed.
    store.request(
        "PUT",
        url,
        data={"info": json.dumps(info)},
        files=files,
        headers={"Accept": "application/json"},
    )
    print(f"Uploaded the banner and {len(media) - ('banner' in listing)} screenshots.")


def main() -> None:
    name = yaml.safe_load((ROOT / "snap/snapcraft.yaml").read_text())["name"]
    listing = yaml.safe_load((ROOT / "snap/local/store-listing.yaml").read_text())
    store = client()
    try:
        sid = snap_id(store, name)
        push_metadata(store, sid, listing)
        push_media(store, sid, listing)
    except craft_store.errors.StoreServerError as e:
        sys.exit(f"Store error: {e}")


if __name__ == "__main__":
    main()
