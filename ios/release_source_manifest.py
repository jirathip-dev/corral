"""Shared manifest and digest logic for the Release build workflow."""

from __future__ import annotations

from hashlib import sha256
from pathlib import Path


RELEASE_SOURCE_FILES = (
    "ios/FleetNotifier/App/AppModel.swift",
    "ios/FleetNotifier/App/FleetNotifierApp.swift",
    "ios/FleetNotifier/App/FleetStore.swift",
    "ios/FleetNotifier/Demo/DemoFleet.swift",
    "ios/FleetNotifier/Keychain/DeviceKeyStore.swift",
    "ios/FleetNotifier/Models/Models.swift",
    "ios/FleetNotifier/Network/CorraldClient.swift",
    "ios/FleetNotifier/Network/SSEParser.swift",
    "ios/FleetNotifier/Notifications/AppDelegate.swift",
    "ios/FleetNotifier/Notifications/LocalNotifier.swift",
    "ios/FleetNotifier/Notifications/PushPayload.swift",
    "ios/FleetNotifier/UI/AppTheme.swift",
    "ios/FleetNotifier/UI/BoardModel.swift",
    "ios/FleetNotifier/UI/FleetViews.swift",
    "ios/FleetNotifier/UI/RecentOutputModel.swift",
    "ios/FleetNotifier/UI/StateStyle.swift",
    "ios/FleetNotifier/UI/TimeInState.swift",
    "ios/FleetNotifier/Wire/CanonicalJSON.swift",
    "ios/FleetNotifier/Wire/DriveClient.swift",
)
SOURCE_DIGEST_PREFIX = "corral-release-source-sha256:"


def release_source_digest(root: Path) -> str:
    """Hash the exact Swift source set compiled into the FleetNotifier app."""

    root = root.resolve()
    digest = sha256()
    for relative in RELEASE_SOURCE_FILES:
        path = root / relative
        if not path.is_file():
            raise FileNotFoundError(path)
        digest.update(relative.encode("utf-8"))
        digest.update(b"\0")
        digest.update(path.read_bytes())
        digest.update(b"\0")
    return digest.hexdigest()


def source_digest_marker(digest: str) -> str:
    """Return the compatibility marker emitted by the declared Release phase."""

    return f"{SOURCE_DIGEST_PREFIX}{digest}"
