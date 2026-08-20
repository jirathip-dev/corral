"""Shared manifest and digest logic for the Release source attestation."""

from __future__ import annotations

from hashlib import sha256
from pathlib import Path


RELEASE_SOURCE_FILES = (
    "ios/FleetNotifier/App/AppModel.swift",
    "ios/FleetNotifier/App/FleetNotifierApp.swift",
    "ios/FleetNotifier/App/FleetStore.swift",
    "ios/FleetNotifier/App/LiveVerifyRunner.swift",
    "ios/FleetNotifier/Demo/DemoFleet.swift",
    "ios/FleetNotifier/Keychain/DeviceKeyStore.swift",
    "ios/FleetNotifier/Models/Models.swift",
    "ios/FleetNotifier/Network/CorraldClient.swift",
    "ios/FleetNotifier/Network/SSEParser.swift",
    "ios/FleetNotifier/Notifications/AppDelegate.swift",
    "ios/FleetNotifier/Notifications/LocalNotifier.swift",
    "ios/FleetNotifier/Notifications/PushPayload.swift",
    "ios/FleetNotifier/Security/Biometrics.swift",
    "ios/FleetNotifier/UI/BoardModel.swift",
    "ios/FleetNotifier/UI/FleetViews.swift",
    "ios/FleetNotifier/Wire/CanonicalJSON.swift",
    "ios/FleetNotifier/Wire/DestructivePatterns.swift",
    "ios/FleetNotifier/Wire/DriveClient.swift",
)
ATTESTATION_PREFIX = "corral-release-source-sha256:"


def release_source_digest(root: Path) -> str:
    """Hash the exact Swift source set compiled into the FleetNotifier app."""

    root = root.resolve()
    discovered = tuple(
        sorted(
            path.relative_to(root).as_posix()
            for path in (root / "ios/FleetNotifier").rglob("*.swift")
        )
    )
    if discovered != RELEASE_SOURCE_FILES:
        raise ValueError(
            "Release source manifest does not match ios/FleetNotifier Swift files"
        )

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


def attestation_marker(digest: str) -> str:
    return f"{ATTESTATION_PREFIX}{digest}"
