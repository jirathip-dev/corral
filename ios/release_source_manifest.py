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
    # #389: the permission posture enum + UNUserNotificationCenter seam is
    # Release app source (the Settings guidance + enable flow compile into
    # Release builds) — added to the manifest.
    "ios/FleetNotifier/Notifications/NotificationPermission.swift",
    "ios/FleetNotifier/Notifications/PushPayload.swift",
    # #399: host-profile store/trust layer — host profiles (id/name/URL/
    # pinned X25519 key + fingerprint/registration metadata), the profile
    # store with legacy migration + remove-host purge, the allowlisted
    # board-cache DTO, the host-key trust helpers, and the push gate.
    "ios/FleetNotifier/Profiles/BoardCache.swift",
    "ios/FleetNotifier/Profiles/HostKeyTrust.swift",
    "ios/FleetNotifier/Profiles/HostProfile.swift",
    "ios/FleetNotifier/Profiles/HostProfileStore.swift",
    # #400: the per-host stream coordinator (composite identity, per-host
    # stream/tail lifecycle + cleanup, offline/stale board projection) is
    # Release app source — added to the manifest.
    "ios/FleetNotifier/Profiles/HostStreamCoordinator.swift",
    "ios/FleetNotifier/Profiles/KeyContinuityGate.swift",
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
