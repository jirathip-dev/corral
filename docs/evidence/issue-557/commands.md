# Exact native gate receipts

Captured subprocess statuses, not pipeline statuses. Full logs remain at the listed paths.

| Gate | Command | Exit | Seconds | Log |
| --- | --- | ---: | ---: | --- |
| parse | `swiftc -parse ios/FleetNotifier/UI/Herd/HerdView.swift ios/FleetNotifierTests/HerdTests.swift` | 0 | 0.83 | `/tmp/g557-parse.log` |
| xcodegen | `xcodegen generate --spec ios/project.yml` | 0 | 0.06 | `/tmp/g557-xcodegen.log` |
| xcodegen-drift | `git diff --exit-code -- ios/FleetNotifier.xcodeproj/project.pbxproj` | 0 | 0.02 | `/tmp/g557-xcodegen-drift.log` |
| release-source | `python3 ios/check-release-demo.py` | 0 | 1.34 | `/tmp/g557-release-source.log` |
| release-self-test | `python3 ios/check-release-demo.py --self-test` | 0 | 68.49 | `/tmp/g557-release-self-test.log` |
| focused | `xcodebuild -project ios/FleetNotifier.xcodeproj -scheme FleetNotifier -configuration Debug -destination 'platform=iOS Simulator,id=0F5C1EDC-64C3-4565-899C-0D7A47CCEF5A' -derivedDataPath /tmp/g557-dd CODE_SIGNING_ALLOWED=NO -parallel-testing-enabled NO -only-testing:FleetNotifierTests/HerdTests -only-testing:FleetNotifierTests/HerdRailZoneTests -only-testing:FleetNotifierTests/FullScreenHerdShellWiringTests test` | 0 | 164.41 | `/tmp/g557-focused.log` |
| debug-build | `xcodebuild -project ios/FleetNotifier.xcodeproj -scheme FleetNotifier -configuration Debug -destination 'generic/platform=iOS Simulator' -derivedDataPath /tmp/g557-dd CODE_SIGNING_ALLOWED=NO -parallel-testing-enabled NO build` | 0 | 30.16 | `/tmp/g557-debug-build.log` |
| release-build | `xcodebuild -project ios/FleetNotifier.xcodeproj -scheme FleetNotifier -configuration Release -destination 'generic/platform=iOS Simulator' -derivedDataPath /tmp/g557-dd CODE_SIGNING_ALLOWED=NO -parallel-testing-enabled NO build` | 0 | 108.54 | `/tmp/g557-release-build.log` |
| release-binary | `python3 ios/check-release-demo.py --binary /tmp/g557-dd/Build/Products/Release-iphonesimulator/FleetNotifier.app/FleetNotifier` | 0 | 4.17 | `/tmp/g557-release-binary.log` |
| slop-base | `swift run --package-path ios/tools/anti-slop-swift --scratch-path /tmp/g557-dd/slop-build anti-slop /tmp/g557-dd/slop-base/ios/FleetNotifier/UI/Herd/HerdView.swift /tmp/g557-dd/slop-base/ios/FleetNotifierTests/HerdTests.swift` | 0 | 100.3 | `/tmp/g557-slop-base.log` |
| slop-head | `swift run --package-path ios/tools/anti-slop-swift --scratch-path /tmp/g557-dd/slop-build anti-slop ios/FleetNotifier/UI/Herd/HerdView.swift ios/FleetNotifierTests/HerdTests.swift` | 0 | 4.35 | `/tmp/g557-slop-head.log` |
| full | `xcodebuild -project ios/FleetNotifier.xcodeproj -scheme FleetNotifier -configuration Debug -destination 'platform=iOS Simulator,id=BD7D5388-56A9-48D4-803C-15003D2D46BD' -derivedDataPath /tmp/g557-dd CODE_SIGNING_ALLOWED=NO -parallel-testing-enabled NO -only-testing:FleetNotifierTests test` | 0 | 264.7 | `/tmp/g557-full.log` |
| mutation-before-sha | `shasum -a 256 ios/FleetNotifier/UI/Herd/HerdView.swift` | 0 | 4.17 | `/tmp/g557-mutation-before-sha.log` |
| mutation-red | `xcodebuild -project ios/FleetNotifier.xcodeproj -scheme FleetNotifier -configuration Debug -destination 'platform=iOS Simulator,id=A784CC8C-088C-44DB-A1D9-E21863AEF388' -derivedDataPath /tmp/g557-dd CODE_SIGNING_ALLOWED=NO -parallel-testing-enabled NO -only-testing:FleetNotifierTests/HerdTests/testHorseCaptionLabelWiresPositiveGitMarkers test` | 65 | 243.78 | `/tmp/g557-mutation-red.log` |
| mutation-restored-sha | `shasum -a 256 ios/FleetNotifier/UI/Herd/HerdView.swift` | 0 | 0.04 | `/tmp/g557-mutation-restored-sha.log` |
| mutation-restore-diff | `git diff --exit-code -- ios/FleetNotifier/UI/Herd/HerdView.swift` | 0 | 0.03 | `/tmp/g557-mutation-restore-diff.log` |
| mutation-green | `xcodebuild -project ios/FleetNotifier.xcodeproj -scheme FleetNotifier -configuration Debug -destination 'platform=iOS Simulator,id=A784CC8C-088C-44DB-A1D9-E21863AEF388' -derivedDataPath /tmp/g557-dd CODE_SIGNING_ALLOWED=NO -parallel-testing-enabled NO -only-testing:FleetNotifierTests/HerdTests/testHorseCaptionLabelWiresPositiveGitMarkers test` | 0 | 20.13 | `/tmp/g557-mutation-green.log` |
| mutation-after-green-sha | `shasum -a 256 ios/FleetNotifier/UI/Herd/HerdView.swift` | 0 | 0.1 | `/tmp/g557-mutation-after-green-sha.log` |
| slop-committed | `swift run --package-path ios/tools/anti-slop-swift --scratch-path /tmp/g557-dd/slop-build anti-slop ios/FleetNotifier/UI/Herd/HerdView.swift ios/FleetNotifierTests/HerdTests.swift` | 0 | 46.79 | `/tmp/g557-slop-committed.log` |
| committed-source-diff | `git diff --exit-code -- ios/FleetNotifier/UI/Herd/HerdView.swift ios/FleetNotifierTests/HerdTests.swift ios/check-release-demo.py` | 0 | 0.3 | `/tmp/g557-committed-source-diff.log` |
| final-diff-check | `git diff --check d5c2b300d5f174d67d8e72f42970403b0f45fcf2` | 0 | 0.46 | `/tmp/g557-final-diff-check.log` |
