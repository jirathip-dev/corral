//
//  SheetPolishTests.swift
//  FleetNotifierTests
//
//  #569 sheet polish evidence + regression tests. These are
//  simulator-native XCTest renders of the REAL RecentOutputSheet hosted over
//  the REAL board (the #558 harness pattern): the production header, the
//  per-state content switch, and the production repo chip are exercised as
//  shipped — nothing here re-implements sheet chrome.
//
//  Two contracts:
//
//  1. Repo identity — the sheet's repo chip paints the SAME hue the Board's
//     `RepoLabelChip` resolves for the same repo (three real repos + the
//     Other/unknown gray). The assertion bites at the PIXEL level: a sheet
//     that reverts to flat muted repo text loses the hue dot and goes RED.
//
//  2. Per-state legibility — every state panel (.loading / .empty /
//     .error / permission) renders its copy directly on the translucent
//     sheet surface with NO backing, and the copy's measured contrast
//     against the ACTUAL rendered backdrop holds WCAG AA in Day and Night.
//     The measurement uses the sheet's own `SheetBackdrop` WCAG math over
//     pixels sampled from the produced frame.
//

import CryptoKit
import SwiftUI
import XCTest
import Vision
@testable import FleetNotifier

@MainActor
final class SheetPolishTests: XCTestCase {

    // MARK: - Fixture

    /// The three real repos the hue contract is verified on (the fleet's
    /// own repo names) plus the Other/unknown case.
    private let realRepos = ["corral", "sendmeter", "synergy-apps"]

    private let worktreeJSON = #"{"repo":"corral","branch":"feature/a-long-branch-for-middle-truncation/sheet","worktree_path":"/fixture/sheet-lane","dirty":true,"ahead":2,"behind":1,"head_sha":"abcdef1234567890","head_subject":"Keep the lane commit context visible without widening the sheet","pr_number":558,"ci_status":"success","issues":[{"number":45},{"number":46},{"number":47}]}"#

    private func agent(id: String, repo: String?, pane: String = "w20Y:p1") -> Agent {
        var workspace = (try? JSONDecoder().decode(Workspace.self,
                                                   from: Data(worktreeJSON.utf8)))
            ?? Workspace(repo: repo, branch: "feature/sheet")
        workspace.repo = repo
        return Agent(agentId: id, source: "herdr", tool: "fixture", state: .idle,
                     reason: "waiting", seq: 1, ts: 1,
                     capabilities: ["read_tail"], workspace: workspace,
                     attachment: Attachment(kind: "herdr", reference: pane),
                     displayName: id, title: nil)
    }

    // MARK: - Harness

    private struct Harness {
        let model: AppModel
        let theme: ThemeStore
        let window: UIWindow
        let presenter: UIHostingController<AnyView>
        let sheet: UIHostingController<AnyView>
        let defaults: UserDefaults
        let suite: String
        let agents: [String: Agent]

        /// Window-space frame of the PRESENTED sheet content (the pixel scan
        /// crops to it so board pixels above the sheet are never counted —
        /// on iOS 26 the presentation container itself is full-window).
        var sheetFrame: CGRect {
            sheet.presentationController?.presentedView?.frame ?? sheet.view.frame
        }

        @MainActor
        func teardown() {
            presenter.dismiss(animated: false)
            window.isHidden = true
            window.rootViewController = nil
            model.stopLive()
            defaults.removePersistentDomain(forName: suite)
        }
    }

    private func makeHarness(flavor: CatppuccinFlavor,
                             type: DynamicTypeSize,
                             agents: [String: Agent],
                             presentFor agentId: String) throws -> Harness {
        let suite = "SheetPolishTests.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        let signer = DeviceSigner(key: Curve25519.Signing.PrivateKey())
        let model = AppModel(defaults: defaults,
                             identityLoader: { (signer, .insecureFallback) },
                             loadMeta: { nil }, saveMeta: { _ in },
                             wipeIdentity: {}, removeMeta: {})
        model.enterDemo()
        model.fleet.seedDemo(agents: agents, rev: 1)
        let theme = ThemeStore(defaults: defaults, reduceMotionProvider: { true })
        theme.setFlavor(flavor)
        let scene = try XCTUnwrap(UIApplication.shared.connectedScenes
            .compactMap { $0 as? UIWindowScene }.first)
        let window = UIWindow(windowScene: scene)
        window.frame = CGRect(x: 0, y: 0, width: 390, height: 844)
        window.overrideUserInterfaceStyle = flavor.isLight ? .light : .dark
        let presenter = UIHostingController(rootView: AnyView(
            FleetView(model: model).environmentObject(theme)))
        window.rootViewController = presenter
        window.makeKeyAndVisible()
        let sheet = UIHostingController(rootView: AnyView(
            RecentOutputSheet(agentId: agentId, hostProfileID: nil, model: model)
                .environmentObject(theme)
                .environment(\.dynamicTypeSize, type)))
        sheet.modalPresentationStyle = .pageSheet
        let presentation = try XCTUnwrap(sheet.sheetPresentationController)
        presentation.detents = [.medium(), .large()]
        presentation.selectedDetentIdentifier = .large
        presenter.present(sheet, animated: false)
        return Harness(model: model, theme: theme, window: window,
                       presenter: presenter, sheet: sheet, defaults: defaults,
                       suite: suite, agents: agents)
    }

    /// Renders the presented window at 3x (the #558/#246 evidence path).
    private func capture(_ harness: Harness) -> UIImage {
        let format = UIGraphicsImageRendererFormat.default()
        format.scale = 3
        return UIGraphicsImageRenderer(bounds: harness.window.bounds, format: format)
            .image { _ in
                harness.window.drawHierarchy(in: harness.window.bounds,
                                             afterScreenUpdates: true)
            }
    }

    private enum StateCase: String, CaseIterable {
        case loading, empty, error, permission

        /// One distinctive copy fragment the frame's OCR must carry.
        var copyFragment: String {
            switch self {
            case .loading: return "Loading recent output"
            case .empty: return "No output yet"
            case .error: return "timeout"
            case .permission: return "Read Tail isn't granted"
            }
        }
    }

    private func failure(_ state: StateCase) -> TranscriptFailure {
        state == .permission
            ? TranscriptFailure(kind: "not_granted", message: "read_tail not granted",
                                candidates: [])
            : TranscriptFailure(kind: "timeout", message: "read_tail timed out",
                                candidates: [])
    }

    private func expectedPhase(_ state: StateCase) -> RecentOutputModel.Phase {
        switch state {
        case .loading: return .loading
        case .empty: return .empty
        case .error, .permission: return .error(failure(state))
        }
    }

    /// Clears every pane (the sheet's own first refresh seeds the demo
    /// tail) and re-seeds the fixture fleet — a state fixture starts empty.
    private func resetAndReseed(_ harness: Harness) {
        harness.model.fleet.reset()
        harness.model.fleet.seedDemo(agents: harness.agents, rev: 1)
    }

    /// Drives the production model into one state AFTER the sheet's own
    /// refresh ran, then proves the sheet's phase resolver agrees with the
    /// intended state before the frame is captured.
    private func drive(_ state: StateCase, harness: Harness, agentId: String) {
        resetAndReseed(harness)
        switch state {
        case .empty:
            break
        case .loading:
            harness.model.fleet.prepareTailFetch(agent: agentId)
        case .error, .permission:
            harness.model.fleet.foldTailFailure(failure(state), for: agentId)
        }
        XCTAssertEqual(RecentOutputModel.phase(for: harness.model.fleet.tailPane(for: agentId)),
                       expectedPhase(state),
                       "the production phase resolver must agree with the fixture's intended state")
    }

    // MARK: - Pixel scan

    private struct PixelGrid {
        let width: Int
        let height: Int
        private let data: [UInt8]

        init(image: UIImage) throws {
            let cg = try XCTUnwrap(image.cgImage)
            width = cg.width
            height = cg.height
            var buffer = [UInt8](repeating: 0, count: width * height * 4)
            let context = try XCTUnwrap(CGContext(
                data: &buffer, width: width, height: height,
                bitsPerComponent: 8, bytesPerRow: width * 4,
                space: CGColorSpaceCreateDeviceRGB(),
                bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue))
            context.draw(cg, in: CGRect(x: 0, y: 0, width: width, height: height))
            data = buffer
        }

        func rgb(_ x: Int, _ y: Int) -> (Int, Int, Int) {
            let index = (y * width + x) * 4
            return (Int(data[index]), Int(data[index + 1]), Int(data[index + 2]))
        }
    }

    private static func components(_ hex: String) -> (Int, Int, Int) {
        let body = hex.hasPrefix("#") ? String(hex.dropFirst()) : hex
        return (Int(body.prefix(2), radix: 16) ?? 0,
                Int(body.dropFirst(2).prefix(2), radix: 16) ?? 0,
                Int(body.dropFirst(4).prefix(2), radix: 16) ?? 0)
    }

    /// Counts exact-ish pixels of one hex inside a point-rect (3x scale).
    private func countPixels(_ grid: PixelGrid, hex: String,
                             in rect: CGRect, tolerance: Int = 1) -> Int {
        let (r, g, b) = Self.components(hex)
        let minX = max(0, Int(rect.minX * 3)), maxX = min(grid.width, Int(rect.maxX * 3))
        let minY = max(0, Int(rect.minY * 3)), maxY = min(grid.height, Int(rect.maxY * 3))
        var count = 0
        for y in minY..<maxY {
            for x in minX..<maxX {
                let (pr, pg, pb) = grid.rgb(x, y)
                if abs(pr - r) <= tolerance, abs(pg - g) <= tolerance,
                   abs(pb - b) <= tolerance {
                    count += 1
                }
            }
        }
        return count
    }

    /// Rows inside `rect` that contain a horizontal run of >= `run` pixels of
    /// this hex. The chip's 6pt hue dot paints an ~18 px solid block at 3x;
    /// text glyphs — and, crucially, their ANTIALIASED edges, which can land
    /// on another palette token by coincidence — never reach that run.
    private func solidRows(_ grid: PixelGrid, hex: String, in rect: CGRect,
                           run: Int = 8, tolerance: Int = 0) -> Int {
        let (r, g, b) = Self.components(hex)
        let minX = max(0, Int(rect.minX * 3)), maxX = min(grid.width, Int(rect.maxX * 3))
        let minY = max(0, Int(rect.minY * 3)), maxY = min(grid.height, Int(rect.maxY * 3))
        var rows = 0
        for y in minY..<maxY {
            var best = 0, current = 0
            for x in minX..<maxX {
                let (pr, pg, pb) = grid.rgb(x, y)
                if abs(pr - r) <= tolerance, abs(pg - g) <= tolerance,
                   abs(pb - b) <= tolerance {
                    current += 1
                    best = max(best, current)
                } else {
                    current = 0
                }
            }
            if best >= run { rows += 1 }
        }
        return rows
    }

    /// The measured WCAG contrast of `inkHex` against the ACTUAL rendered
    /// backdrop: the ink pixels are located in the sheet region, and the
    /// backdrop is the median of the non-ink pixels on the ink's own rows.
    private func measuredContrast(_ image: UIImage, sheetFrame: CGRect,
                                  inkHex: String) throws
        -> (ratio: Double, backdrop: String, inkPixels: Int) {
        let grid = try PixelGrid(image: image)
        let (ir, ig, ib) = Self.components(inkHex)
        let minX = max(0, Int(sheetFrame.minX * 3)), maxX = min(grid.width, Int(sheetFrame.maxX * 3))
        let minY = max(0, Int(sheetFrame.minY * 3)), maxY = min(grid.height, Int(sheetFrame.maxY * 3))
        var inkPoints: [(Int, Int)] = []
        for y in minY..<maxY {
            for x in minX..<maxX {
                let (r, g, b) = grid.rgb(x, y)
                if abs(r - ir) <= 8, abs(g - ig) <= 8, abs(b - ib) <= 8 {
                    inkPoints.append((x, y))
                }
            }
        }
        XCTAssertGreaterThan(inkPoints.count, 40,
                             "the state copy ink (\(inkHex)) must render in the sheet region")
        let firstY = inkPoints.map(\.1).min() ?? minY
        let lastY = inkPoints.map(\.1).max() ?? maxY
        var reds: [Int] = [], greens: [Int] = [], blues: [Int] = []
        for y in max(minY, firstY - 2)...min(maxY - 1, lastY + 2) {
            for x in minX..<maxX {
                let (r, g, b) = grid.rgb(x, y)
                if abs(r - ir) <= 12, abs(g - ig) <= 12, abs(b - ib) <= 12 { continue }
                reds.append(r); greens.append(g); blues.append(b)
            }
        }
        XCTAssertGreaterThan(reds.count, 200, "the backdrop sample must be non-trivial")
        func median(_ values: [Int]) -> Int { values.sorted()[values.count / 2] }
        let backdrop = String(format: "#%02x%02x%02x",
                              median(reds), median(greens), median(blues))
        return (SheetBackdrop.contrastRatio(inkHex, backdrop), backdrop, inkPoints.count)
    }

    private func recognizedText(_ image: UIImage) throws -> String {
        let request = VNRecognizeTextRequest()
        request.recognitionLevel = .accurate
        request.recognitionLanguages = ["en-US"]
        try VNImageRequestHandler(cgImage: XCTUnwrap(image.cgImage)).perform([request])
        return (request.results ?? []).compactMap { $0.topCandidates(1).first?.string }
            .joined(separator: " ")
    }

    private func save(_ image: UIImage, name: String) throws -> URL {
        let attachment = XCTAttachment(image: image)
        attachment.name = "569-" + name
        attachment.lifetime = .keepAlways
        add(attachment)
        let directory = try XCTUnwrap(FileManager.default.urls(for: .documentDirectory,
                                                               in: .userDomainMask).first)
            .appendingPathComponent("g569-evidence")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let url = directory.appendingPathComponent(name + ".png")
        try XCTUnwrap(image.pngData()).write(to: url)
        return url
    }

    // MARK: - 1. Repo identity (the bite)

    /// The sheet's repo chip resolves the same hue as the Board's
    /// `RepoLabelChip` for three real repos plus the Other/unknown gray —
    /// asserted on the RENDERED PIXELS of both surfaces. A sheet that
    /// reverts to flat muted repo text has no hue dot: the count collapses
    /// to zero and this test goes RED.
    func testSheetRepoChipPaintsTheBoardHueForThreeReposAndOther() async throws {
        var fixtureAgents: [String: Agent] = [:]
        for repo in realRepos { fixtureAgents["herdr:\(repo)"] = agent(id: "herdr:\(repo)", repo: repo) }
        fixtureAgents["herdr:unknown"] = agent(id: "herdr:unknown", repo: nil)
        let repos = BoardModel.repoFilters(Array(fixtureAgents.values)).map(\.repo)
        XCTAssertEqual(repos, realRepos.sorted(),
                       "the fixture fleet's repo set is the three real repos")

        for flavor in [CatppuccinFlavor.latte, .mocha] {
            let theme = ThemeStore(defaults: try XCTUnwrap(UserDefaults(
                suiteName: "SheetPolishTests.chip.\(UUID().uuidString)")),
                                   reduceMotionProvider: { true })
            theme.setFlavor(flavor)
            var expected: [String: String] = [:]   // agent id -> dot hex
            for (id, fixture) in fixtureAgents {
                let hue = theme.repoHue(for: fixture.workspace.repo ?? "", among: repos)
                let dotHex = theme.color(hue).hexDescription
                expected[id] = dotHex

                // The BOARD chip for the same repo: its real component
                // render must carry the same dot hue (the control that the
                // expectation is the Board's own resolution).
                let boardChip = ImageRenderer(content: RepoLabelChip(
                    repo: fixture.workspace.repo, repos: repos)
                    .padding(8)
                    .background(theme.base)
                    .environmentObject(theme).environment(\.dynamicTypeSize, .large))
                boardChip.scale = 3
                let chipGrid = try PixelGrid(image: try XCTUnwrap(boardChip.uiImage))
                let chipDots = countPixels(chipGrid, hex: dotHex,
                                           in: CGRect(x: 0, y: 0, width: 200, height: 40))
                let chipRows = solidRows(chipGrid, hex: dotHex,
                                         in: CGRect(x: 0, y: 0, width: 200, height: 40),
                                         tolerance: 1)
                print("G569_BOARD_CHIP flavor=\(flavor.rawValue) repo=\(fixture.workspace.repo ?? "Other") hue=\(dotHex) dot_pixels=\(chipDots) dot_rows=\(chipRows)")
                XCTAssertGreaterThan(chipRows, 6,
                                     "the Board chip's dot for \(fixture.workspace.repo ?? "Other") must paint \(dotHex) as a solid block")
                if fixture.workspace.repo == nil {
                    XCTAssertEqual(hue, .surface2, "Other/unknown must resolve the surface2 gray")
                    XCTAssertFalse(RepoHue.ring.contains(hue),
                                   "Other must never take an accent ring hue")
                }
            }
            // Present each fixture agent's sheet; the sheet's chip dot must
            // carry the SAME hex the Board chip painted.
            for (id, fixture) in fixtureAgents {
                let harness = try makeHarness(flavor: flavor, type: .large,
                                              agents: fixtureAgents, presentFor: id)
                try await Task.sleep(for: .milliseconds(700))
                harness.window.layoutIfNeeded()
                let image = capture(harness)
                let label = fixture.workspace.repo ?? "Other"
                _ = try save(image, name: "chip-\(flavor.rawValue)-\(fixture.workspace.repo ?? "other")")
                let grid = try PixelGrid(image: image)
                // The chip lives in the sheet's header band: scan that band
                // only, so nothing below the divider (the loaded demo
                // stream's ANSI colors, for instance) can satisfy or
                // pollute a hue count.
                let band = CGRect(x: harness.sheetFrame.minX, y: harness.sheetFrame.minY,
                                  width: harness.sheetFrame.width,
                                  height: min(140, harness.sheetFrame.height))
                let mine = countPixels(grid, hex: try XCTUnwrap(expected[id]), in: band)
                let mineRows = solidRows(grid, hex: try XCTUnwrap(expected[id]), in: band,
                                         tolerance: 1)
                print("G569_CHIP flavor=\(flavor.rawValue) repo=\(label) hue=\(expected[id] ?? "") dot_pixels=\(mine) dot_rows=\(mineRows) band=\(band) crop=\(harness.sheetFrame)")
                XCTAssertGreaterThan(mineRows, 6,
                                     "the sheet's repo chip for \(label) must paint \(expected[id] ?? "") — a 6pt hue dot is a solid block; flat muted text has no dot")
                for (otherId, otherHex) in expected where otherId != id {
                    let foreignRows = solidRows(grid, hex: otherHex, in: band)
                    XCTAssertLessThanOrEqual(foreignRows, 2,
                                             "\(otherId)'s hue must not paint a solid block in \(id)'s sheet header band")
                }
                harness.teardown()
            }
        }
    }

    // MARK: - 2. Per-state renders + measured contrast

    /// One frame per state with header → state → worktree block visible,
    /// Day (Latte) and Night (Mocha) at normal and AX3 Dynamic Type. Each
    /// frame's copy is measured against the ACTUAL rendered backdrop with
    /// the sheet's own WCAG math and must hold AA (4.5:1 text, 3:1 for the
    /// error's warning glyph, which is non-text).
    func testRenderedSheetStatesDayNightNormalAndLargeText() async throws {
        let fixtureAgents = ["herdr:sheet-fixture": agent(id: "herdr:sheet-fixture", repo: "corral")]
        var frames = 0
        for flavor in [CatppuccinFlavor.latte, .mocha] {
            for type in [DynamicTypeSize.large, .accessibility3] {
                for state in StateCase.allCases {
                    let harness = try makeHarness(flavor: flavor, type: type,
                                                  agents: fixtureAgents,
                                                  presentFor: "herdr:sheet-fixture")
                    // Let the sheet's own first refresh run, then drive the
                    // state so the frame shows THIS state's panel.
                    try await Task.sleep(for: .milliseconds(900))
                    drive(state, harness: harness, agentId: "herdr:sheet-fixture")
                    harness.window.layoutIfNeeded()
                    try await Task.sleep(for: .milliseconds(350))
                    harness.window.layoutIfNeeded()
                    let image = capture(harness)
                    let size = type == .accessibility3 ? "ax3" : "default"
                    let name = "\(flavor == .latte ? "day" : "night")-\(size)-\(state.rawValue)"
                    _ = try save(image, name: name)
                    frames += 1

                    // Header → state → worktree block all in this frame.
                    let text = try recognizedText(image)
                    for fragment in ["corral", "abcdef1", "sheet-lane", "passing",
                                     state.copyFragment] {
                        XCTAssertTrue(text.contains(fragment),
                                      "\(name) must show \(fragment); OCR: \(text)")
                    }

                    // Measured contrast of the copy (and of the error's
                    // warning glyph) over the actual rendered backdrop.
                    let ink = harness.theme.text.hexDescription
                    let measured = try measuredContrast(image, sheetFrame: harness.sheetFrame,
                                                        inkHex: ink)
                    print(String(format: "G569_CONTRAST state=%@ flavor=%@ size=%@ ink=%@ backdrop=%@ ratio=%.2f crop=%@",
                                 state.rawValue, flavor.rawValue, size, ink,
                                 measured.backdrop, measured.ratio,
                                 String(describing: harness.sheetFrame)))
                    XCTAssertGreaterThanOrEqual(measured.ratio, SheetBackdrop.minimumContrast,
                                                "\(name): the state copy must hold AA over the rendered backdrop")
                    if state == .error {
                        let glyph = harness.theme.codeDeletion.hexDescription
                        let glyphMeasured = try measuredContrast(image,
                                                                 sheetFrame: harness.sheetFrame,
                                                                 inkHex: glyph)
                        print(String(format: "G569_CONTRAST state=error-glyph flavor=%@ size=%@ ink=%@ backdrop=%@ ratio=%.2f",
                                     flavor.rawValue, size, glyph,
                                     glyphMeasured.backdrop, glyphMeasured.ratio))
                        XCTAssertGreaterThanOrEqual(glyphMeasured.ratio, 3.0,
                                                    "\(name): the warning glyph is non-text and must hold 3:1")
                    }
                    harness.teardown()
                }
            }
        }
        XCTAssertEqual(frames, 16, "two flavors x two type sizes x four states")
    }

    // MARK: - 4. The tier, stated with the repo's own worst-case math

    /// Why the state copy rides the `text` tier: against the #416 locked
    /// 80 %-tinted translucent backdrop, the AA tier clears 4.5:1 in the
    /// worst underlying-content case in BOTH flavors while the muted/dim
    /// tiers (and red as text) do not — that is exactly the legibility debt
    /// the #428 opaque backing used to pay, and the numbers behind the
    /// owner's "drop the panel background" decision. Prints every tier's
    /// worst-case ratio for the record.
    func testStateCopyTierHoldsAAOverTheWorstCaseTintedBackdrop() throws {
        for flavor in [CatppuccinFlavor.latte, .mocha] {
            let theme = ThemeStore(defaults: try XCTUnwrap(UserDefaults(
                suiteName: "SheetPolishTests.tier.\(UUID().uuidString)")),
                                   reduceMotionProvider: { true })
            theme.setFlavor(flavor)
            let palette = CatppuccinPalette.palette(for: flavor)
            let candidates = CatppuccinToken.allCases.map { palette.hex($0) }
            func worst(_ ink: Color) -> Double {
                SheetBackdrop.worstContrast(ink: ink.hexDescription,
                                            tint: palette.hex(.base), over: candidates)
            }
            print(String(format: "G569_TIER flavor=%@ text=%.2f tailMuted=%.2f tailQuiet=%.2f red=%.2f mauve=%.2f",
                         flavor.rawValue, worst(theme.text), worst(theme.tailMuted),
                         worst(theme.tailQuiet), worst(theme.codeDeletion), worst(theme.accent)))
            XCTAssertGreaterThanOrEqual(worst(theme.text), SheetBackdrop.minimumContrast,
                                        "\(flavor.rawValue): the state copy's tier must clear "
                                        + "4.5:1 over the 80 % tinted backdrop in the worst "
                                        + "underlying-content case (no backing to hide behind)")
        }
    }

    // MARK: - 5. No slab, one grid (source pin over the bundled sheet)

    /// The sheet's state panels paint NO background and every state keeps
    /// the header's 16/10 grid — a future lane that reintroduces
    /// `.padding(16)` + `.background(theme.base)` in a state panel goes RED.
    func testStatePanelsPaintNoBackingAndKeepTheHeaderGrid() throws {
        let bundle = Bundle(for: SheetPolishTests.self)
        let url = try XCTUnwrap(bundle.url(forResource: "FleetViews", withExtension: "swift.txt"))
        let source = try String(contentsOf: url, encoding: .utf8)
        let sheetStart = try XCTUnwrap(source.range(of: "struct RecentOutputSheet: View {"))
        let sheetEnd = try XCTUnwrap(source.range(of: "\n// MARK: - Recents block renderer"))
        let sheet = String(source[sheetStart.lowerBound..<sheetEnd.lowerBound])
        XCTAssertFalse(sheet.contains(".padding(16)"),
                       "the #569 slab padding is gone from the sheet")
        XCTAssertEqual(sheet.components(separatedBy: ".padding(.horizontal, 16)").count - 1, 5,
                       "the header band + all four state panels (loading/empty/permission/error) "
                       + "keep the 16pt horizontal grid")
        XCTAssertEqual(sheet.components(separatedBy: ".padding(.vertical, 10)").count - 1, 6,
                       "the header band, the four state panels and the loaded block stream "
                       + "all keep the 10pt vertical grid")
        XCTAssertEqual(sheet.components(separatedBy: ".background(theme.base)").count - 1, 1,
                       "only the #428 header band keeps an opaque base backing — no state slab")
        XCTAssertTrue(sheet.contains("RepoLabelChip(repo: agent.workspace.repo, repos: repos, compact: true)"),
                      "the sheet's repo identity is the Board's hue chip component, in its "
                      + "COMPACT variant so the caption row keeps the pre-#569 height the "
                      + "#558 commit line's render depends on")
        XCTAssertFalse(sheet.contains("Text(repo)"),
                       "the flat muted repo label is gone (the chip carries the identity)")
        XCTAssertTrue(sheet.contains(".accessibilityLabel(\"Pane \\(reference)\")"),
                      "the pane capsule keeps the pane reference's spoken label")
    }
}
