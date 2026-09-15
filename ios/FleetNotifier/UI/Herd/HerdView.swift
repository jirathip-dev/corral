import SwiftUI

struct HerdView: View {
    let horses: [HerdHorse]
    let obscured: Bool
    /// #456: the board's reconciled scope labels (the SAME projections the
    /// board's Filters control uses) rendered by the floating top chrome.
    let scopeLabel: String
    let scopeSummary: String
    /// #528: the ONE compact connection indicator state — the SAME model the
    /// Board chrome renders, so both modes report the identical fleet
    /// connection truth (no outage panel, no duplicated copy).
    let connection: BoardModel.ConnectionIndicatorModel
    /// #456: the floating chrome drives the SAME sheets the board chrome
    /// does — the bindings are FleetView's own presentation state, so Herd
    /// never owns a parallel sheet or a duplicated toolbar.
    @Binding var showConnectionDetail: Bool
    @Binding var showFilters: Bool
    @Binding var showSettings: Bool
    /// #457: the ranch lighting (night flag) the floating chrome resolved.
    /// HerdView is the single `HerdSun` resolver site; it reports the value
    /// up so the shared filter sheet's Herd context is styled from the SAME
    /// lighting the trigger and the counts show. Inert by default for
    /// standalone hosts (tests/previews).
    let onLightingNight: (Bool) -> Void
    let select: (HerdHorse) -> Void
    @EnvironmentObject private var theme: ThemeStore
    @Environment(\.scenePhase) private var scenePhase
    @Environment(\.accessibilityReduceMotion) private var systemReduceMotion
    @Environment(\.dynamicTypeSize) private var dynamicType
    @AppStorage("herdEnvironment") private var environment = HerdEnvironmentChoice.auto
    @StateObject var clock = HerdClock()
    @StateObject private var location = HerdLocation()
    @State var sceneID = UUID()
    @State var paddockID: String?
    @State var scroll: CGFloat = 0
    @State private var now = Date()
    @State private var timeRevision = 0
    @State private var dragging = false
    private let hudSpacing: CGFloat = 6
    init(horses: [HerdHorse], obscured: Bool,
         scopeLabel: String = "Filters", scopeSummary: String = "All repositories",
         connection: BoardModel.ConnectionIndicatorModel
             = BoardModel.connectionIndicator(hosts: []),
         showConnectionDetail: Binding<Bool> = .constant(false),
         showFilters: Binding<Bool> = .constant(false),
         showSettings: Binding<Bool> = .constant(false),
         onLightingNight: @escaping (Bool) -> Void = { _ in },
         select: @escaping (HerdHorse) -> Void,
         clock: HerdClock? = nil) {
        self.horses = horses
        self.obscured = obscured
        self.scopeLabel = scopeLabel
        self.scopeSummary = scopeSummary
        self.connection = connection
        _showConnectionDetail = showConnectionDetail
        _showFilters = showFilters
        _showSettings = showSettings
        self.onLightingNight = onLightingNight
        self.select = select
        _clock = StateObject(wrappedValue: clock ?? HerdClock())
    }
#if DEBUG
    @State var evidenceEnvironment: HerdEnvironmentChoice?
    @State var evidenceElapsed: Double?
    @State var evidenceReduceMotion = false
    @State var evidenceRan = false
    @State var evidencePhase: String?
    @State var evidenceFullScreenRan = false
    /// #526 evidence: shifts the lighting instant (see `lightingNow`) so the
    /// Auto day→night transition can be captured deterministically. nil in
    /// every non-evidence launch; Release never compiles this.
    @State var evidenceClockOffset: TimeInterval?
#endif
    private var paddocks: [HerdPaddock] { HerdProjection.paddocks(horses) }
    private var rail: [HerdHorse] { horses.filter(\.atRail) }
    var reduced: Bool {
#if DEBUG
        if evidenceReduceMotion { return true }
#endif
        return theme.reduceMotion || systemReduceMotion
    }
    var effectiveEnvironment: HerdEnvironmentChoice {
#if DEBUG
        if let evidenceEnvironment { return evidenceEnvironment }
#endif
        return environment
    }
    /// #526: the instant the lighting resolves against. The live clock in
    /// production; the #526 evidence driver shifts it (`evidenceClockOffset`)
    /// to exercise a REAL Auto day→night transition deterministically —
    /// the resolved `lighting.night` then drives the ranch, the chrome and
    /// the ThemeStore through the same production path a boundary crossing
    /// takes.
    var lightingNow: Date {
#if DEBUG
        if let evidenceClockOffset { return now.addingTimeInterval(evidenceClockOffset) }
#endif
        return now
    }
    var lighting: HerdLighting {
        HerdSun.resolve(effectiveEnvironment,now:lightingNow,location:location.sample())
    }
    /// #457: the sealed ranch Day/Night control palette — resolved from the
    /// SAME `lighting` the ranch field renders with, so the floating chrome
    /// and the environment can never disagree.
    var ranchTokens: RanchControlTokens {
        .resolve(night: lighting.night)
    }
    var elapsed: Double {
#if DEBUG
        if let evidenceElapsed { return evidenceElapsed }
#endif
        return clock.elapsed
    }
    var motionEnabled: Bool {
        scenePhase == .active && !obscured && !reduced && horses.contains { !$0.disconnected }
    }
    private var solarKey: String {
        "\(effectiveEnvironment.rawValue)-\(location.revision)-\(timeRevision)-\(scenePhase == .active)"
    }
    var body: some View {
        // #456: the procedural ranch is the full-screen ROOT layer, painted
        // behind every safe area; the floating chrome + content render above
        // it (the ranch already refuses hit testing), so Day/Night covers the
        // whole Herd surface instead of a strip under an opaque board header.
        // The cover gives the approved 390x640 world ONE uniform scale for
        // both axes — no per-axis stretch, cropped by the screen.
        ZStack {
            GeometryReader { screen in
                HerdRanchCover {
                    RanchEnvironment(night:lighting.night,scroll:scroll,
                                     maxScroll:CGFloat(max(0,paddocks.count-1))*screen.size.width,
                                     elapsed:elapsed,reduceMotion:!motionEnabled)
                }
            }
            .ignoresSafeArea()
            VStack(spacing:0) {
                topChrome
                    // #456-r1: at accessibility sizes the chrome's ideal height
                    // grows with the type. It is the surface that must never be
                    // squeezed (the pre-fix squeeze drew the scope label outside
                    // its own pill and under the counts card), so it keeps its
                    // Dynamic-Type ideal and the pager region below absorbs the
                    // difference instead.
                    .layoutPriority(1)
                GeometryReader { geometry in
                    VStack(spacing:0) {
                        if paddocks.isEmpty {
                            ContentUnavailableView("No agents in this scope",systemImage:"line.3.horizontal.decrease")
                        } else {
                            herdColumn(width:geometry.size.width)
                            navigation
                                // #456-r1: Previous/Next/position keep their
                                // >= 44 pt targets at accessibility sizes too.
                                .layoutPriority(1)
                        }
                    }
                }
            }
        }
        .onAppear { if motionEnabled { clock.start() } }
        // #457/#526: report the resolved lighting up to FleetView — the
        // shared filter sheet's Herd context styles itself from this value,
        // and the ROOT pushes it into the ThemeStore (so the resolved
        // Day/Night state drives the whole app palette from this ONE
        // resolver, and this leaf view never writes the app-level store).
        // `initial: true` covers the first rendered frame.
        .onChange(of:lighting.night,initial:true) { _,night in
            onLightingNight(night)
        }
        .onChange(of:motionEnabled) { _,enabled in
            if enabled { clock.start() } else { clock.stop() }
        }
        .onDisappear {
            clock.stop()
#if DEBUG
            HerdEvidence.record("dismissed",scene:self)
            HerdEvidence.observeDismissal(scene:self)
#endif
        }
        .task(id:solarKey) {
            guard scenePhase == .active else { return }
            while !Task.isCancelled {
                now = Date()
                let delay = max(1,lighting.nextChange.timeIntervalSince(now))
                do { try await Task.sleep(for:.seconds(delay)) }
                catch { return }
            }
        }
        .onReceive(NotificationCenter.default.publisher(for:UIApplication.significantTimeChangeNotification)) { _ in timeRevision += 1 }
        .onReceive(NotificationCenter.default.publisher(for:.NSSystemTimeZoneDidChange)) { _ in timeRevision += 1 }
        .onChange(of:paddocks.map(\.id),initial:true) { _,ids in
            paddockID = HerdProjection.reconciledPaddockID(paddockID,in:ids)
        }
#if DEBUG
        .coordinateSpace(name:"herdRailLayout")
        .task {
            // #456 full-screen evidence supersedes the #459-era sequence in
            // the same launch (one deterministic marker stream).
            if !CommandLine.arguments.contains("-corral456FullScreenEvidence") {
                await runHerdEvidence()
            }
        }
        .task { await runFullScreenEvidence() }
        // #526 evidence: the Auto transition + pagination phases (HerdView
        // owns the pager and the lighting instant they drive).
        .task { await runPaletteAutoEvidence() }
        // Record from the current rendered value, not the task's captured
        // View struct (whose environment/immutable horse props may be stale).
        .onChange(of:evidencePhase) { _,phase in
            if let phase { HerdEvidence.record(phase,scene:self) }
        }
#endif
    }
    /// #456: the floating top chrome — scope + Settings on the first row,
    /// truthful scoped counts + the environment explanation beneath. Each
    /// surface floats over the ranch; #457 moves the scope pill, the
    /// Settings control and the counts card onto the sealed ranch Day/Night
    /// chrome (glass where available, opaque Reduce Transparency /
    /// high-contrast fallback) so no light/dark text is inherited blindly
    /// from the app flavor. Targets stay >= 44 pt with a safe-area-aware
    /// top margin. #528: the ONE compact connection indicator rides this
    /// SAME row, between the scope pill and the gear — the removed
    /// disconnect panel's recovery actions + verbose copy are gone.
    private var topChrome: some View {
        VStack(spacing:hudSpacing) {
            HStack(spacing:8) {
                scopeControl
                ConnectionStatusIndicator(model: connection,
                                          palette: ConnectionIndicatorPalette(ranch: ranchTokens,
                                                                              theme: theme),
                                          detailChrome: ranchTokens,
                                          showDetail: $showConnectionDetail)
                    .ranchChromeSurface(ranchTokens)
                settingsControl
            }
#if DEBUG
            .modifier(HerdRailFrameProbe(name:"hud-row"))
#endif
            statusSummary
#if DEBUG
                .modifier(HerdRailFrameProbe(name:"hud-counts"))
#endif
        }
        .padding(.horizontal,12)
        .padding(.top,6)
    }
    /// The scope control repeats the board Filters control's contract over
    /// the ranch: same reconciled labels, same sheet, one >= 44 pt button —
    /// presented in the ranch Day/Night chrome (#457).
    private var scopeControl: some View {
        Button { showFilters = true } label: {
            HStack(spacing:8) {
                HerdFilterGlyph(color: ranchTokens.accentColor)
                    .frame(width:16,height:16)
                VStack(alignment:.leading,spacing:1) {
                    Text(scopeLabel).font(.subheadline.weight(.semibold))
                        .foregroundStyle(ranchTokens.inkColor).lineLimit(1)
                    Text(scopeSummary).font(.caption2).foregroundStyle(ranchTokens.mutedColor)
                        .lineLimit(1).truncationMode(.tail)
                }
                Spacer(minLength:0)
            }
            .frame(maxWidth:.infinity,minHeight:44,alignment:.leading)
            .padding(.horizontal,12)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .ranchChromeSurface(ranchTokens)
        .accessibilityElement(children:.ignore)
        .accessibilityLabel(scopeLabel + ", " + scopeSummary)
        .accessibilityHint("Opens host and repository filters")
    }
    /// #456: Settings stays reachable in Herd (the navigation toolbar is
    /// hidden here, so this floating gear is the mode's only gear — never a
    /// duplicated toolbar). #457: it rides the same ranch chrome as the
    /// trigger it sits beside.
    private var settingsControl: some View {
        Button { showSettings = true } label: {
            HerdGearGlyph(color: ranchTokens.inkColor)
                .frame(width:22,height:22)
                .frame(width:44,height:44)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .ranchChromeSurface(ranchTokens)
        .accessibilityLabel("Settings")
        .accessibilityHint("Opens connection and notification settings")
    }
    /// #491 (owner-adaptive): the counts summary is an adaptive fit ladder —
    /// full labels on one row where they fit, the compact mark+count row
    /// where that fits, and the vertical list (full entries, else compact)
    /// at very large Dynamic Type where no single row can fit. Never tiny
    /// type, clipping, ellipsis, count caps or horizontal scrolling. The
    /// Day/Night explanation line is deliberately gone from this bar; the
    /// environment control, its transitions and the ambience stay.
    private var statusSummary: some View {
        ViewThatFits(in:.horizontal) {
            HStack(spacing:8) { counts }
            HStack(spacing:8) { compactCounts }
            VStack(alignment:.leading,spacing:2) { counts }
            VStack(alignment:.leading,spacing:2) { compactCounts }
        }
        .foregroundStyle(ranchTokens.inkColor)
        .padding(.vertical,6).padding(.horizontal,12)
        .frame(maxWidth:.infinity)
        .ranchChromeSurface(ranchTokens)
        .accessibilityElement(children:.ignore)
        .accessibilityLabel(herdCountsAccessibilityLabel(horses))
    }
    @ViewBuilder private var counts: some View {
        ForEach([AgentState.blocked,.working,.idle,.done,.unknown],id:\.self) { state in
            Text("\(herdMark(state)) \(horses.filter { $0.state == state }.count) \(state.rawValue)")
                .font(.system(.caption2,design:.monospaced))
        }
    }
    /// #491: the compact single-row representation — mark + exact count in
    /// the same caption2 monospaced type; the full status names stay in the
    /// summary's VoiceOver label.
    @ViewBuilder private var compactCounts: some View {
        ForEach([AgentState.blocked,.working,.idle,.done,.unknown],id:\.self) { state in
            Text("\(herdMark(state)) \(horses.filter { $0.state == state }.count)")
                .font(.system(.caption2,design:.monospaced))
        }
    }
    /// #528: the old disconnect panel (the source-disconnected copy plus
    /// its recovery actions) is REMOVED — the compact connection indicator
    /// in the floating chrome carries the state, and the stream/model
    /// recover on their own (the board uses the same source, so switching
    /// view was never network recovery). Disconnected horses still render
    /// their last-known truth (`unknown · last known …`) via HerdHorse.
    private var railCardWidth: CGFloat { dynamicType.isAccessibilitySize ? 240 : 164 }

    /// #548: measure the card's intrinsic height, including the longest rail
    /// status (and a host line in multi-host scopes). No art or hit target;
    /// occupancy never participates in its layout. Dynamic Type still does.
    private var railZone: some View {
        VStack(spacing:2) {
            Color.clear.frame(height:108)
            horseCaption(name:" ",status:"? unknown · last known blocked",
                         hostName:horses.contains { $0.hostName != nil } ? " " : nil,rail:true)
        }
        .frame(width:railCardWidth)
        .fixedSize(horizontal:false,vertical:true)
        .hidden()
        .frame(maxWidth:.infinity,alignment:.leading)
        .overlay(alignment:.topLeading) {
            if !rail.isEmpty {
                ScrollView(.horizontal) {
                    LazyHStack(alignment:.top,spacing:10) {
                        ForEach(rail) { horse in horseButton(horse,rail:true) }
                    }.padding(.horizontal,12)
                }.scrollIndicators(.hidden)
            }
        }
        .accessibilityElement(children:.contain)
        .accessibilityLabel("Global blocked front rail")
        .accessibilityHidden(rail.isEmpty)
#if DEBUG
        .modifier(HerdRailFrameProbe(name:"rail-zone"))
#endif
    }

    private var repositoryChip: some View {
        let paddock = paddocks.first { $0.id == paddockID } ?? paddocks.first
        return Group {
            if let paddock {
                let name = Text(paddock.title).font(.subheadline.weight(.semibold))
                    .foregroundColor(ranchTokens.inkColor)
                let counts = Text(herdRepositoryCaption(paddock).dropFirst(paddock.title.count))
                    .font(.caption).foregroundColor(ranchTokens.mutedColor)
                ZStack(alignment:.leading) {
                    if dynamicType.isAccessibilitySize {
                        // Reserve two lines across rail occupancy changes, without
                        // capping longer names or counts at a truncating line limit.
                        Text("\n").font(.subheadline.weight(.semibold)).hidden()
                            .accessibilityHidden(true)
                    }
                    Text("\(name)\(counts)")
                        .fixedSize(horizontal:false,vertical:true)
#if DEBUG
                        .modifier(HerdRailFrameProbe(name:"repository-text"))
#endif
                }
                .padding(.horizontal,12).padding(.vertical,6)
                .ranchChromeSurface(ranchTokens)
                .accessibilityLabel(herdRepositoryCaption(paddock))
#if DEBUG
                .modifier(HerdRailFrameProbe(name:"repository-surface"))
#endif
            }
        }
        .frame(maxWidth:.infinity,alignment:.leading)
        .padding(.horizontal,12).padding(.top,hudSpacing)
#if DEBUG
        .modifier(HerdRailFrameProbe(name:"repository-chip"))
#endif
    }

    /// The same reserved zone in both paths. Large type scrolls the entire
    /// chip/rail/field column between the pinned HUD and navigation.
    @ViewBuilder private func herdColumn(width:CGFloat) -> some View {
        let column = VStack(spacing:0) {
            repositoryChip
            railZone.layoutPriority(1)
            pager(width:width)
        }
        if dynamicType.isAccessibilitySize {
            ScrollView(.vertical) { column }
        } else {
            column
        }
    }
    private func pager(width:CGFloat) -> some View {
        ScrollView(.horizontal) {
            LazyHStack(spacing:0) {
                ForEach(paddocks) { paddock in
                    Group {
                        if dynamicType.isAccessibilitySize {
                            paddockField(paddock,width:width)
                        } else {
                            ScrollView(.vertical) { paddockField(paddock,width:width) }
                        }
                    }.padding(.horizontal,12).frame(width:width).id(paddock.id)
                        .background(GeometryReader { proxy in
                            let index = paddocks.firstIndex { $0.id == paddock.id } ?? 0
                            Color.clear.preference(key:HerdScrollOffset.self,
                                value:CGFloat(index)*width-proxy.frame(in:.named("herdPager")).minX)
                        })
                }
            }
            .scrollTargetLayout()

        }
        .coordinateSpace(name:"herdPager")
        .scrollTargetBehavior(.paging)
        .scrollPosition(id:$paddockID)
        .scrollIndicators(.hidden)
        .modifier(HerdScrollTracking(offset: $scroll))
        .simultaneousGesture(DragGesture().onChanged { _ in dragging = true }.onEnded { _ in dragging = false })
    }
    /// #456: compact floating paddock navigation above the home indicator —
    /// Previous / position / Next as a rounded pill instead of the old
    /// opaque full-width bar. #526: the pill, its text and its
    /// enabled/disabled control inks ride the SAME ranch Day/Night chrome
    /// the rest of the Herd surface renders with (the #457 sealed tokens),
    /// so no surface inherits the app flavor over the ranch.
    private var navigation: some View {
        let index = paddocks.firstIndex { $0.id == paddockID } ?? 0
        return HStack {
            Button("Previous") { movePage(-1) }.disabled(index == 0).frame(minWidth:44,minHeight:44)
                .foregroundStyle(index == 0 ? ranchTokens.mutedColor : ranchTokens.inkColor)
                .accessibilityHint(index == 0 ? "First repository paddock" : "Previous repository paddock")
            Spacer(minLength:4)
            Text("\(index+1) / \(paddocks.count)").font(.caption.monospacedDigit())
            Spacer(minLength:4)
            Button("Next") { movePage(1) }.disabled(index+1 >= paddocks.count).frame(minWidth:44,minHeight:44)
                .foregroundStyle(index+1 >= paddocks.count ? ranchTokens.mutedColor : ranchTokens.inkColor)
                .accessibilityHint(index+1 >= paddocks.count ? "Last repository paddock" : "Next repository paddock")
        }.font(.caption)
            .foregroundStyle(ranchTokens.inkColor)
            .padding(.horizontal,12).padding(.vertical,6)
            .ranchChromeSurface(ranchTokens)
            .padding(.horizontal,12).padding(.bottom,6)
            .accessibilityLabel("Repository paddocks")
    }
    func movePage(_ delta:Int) {
        let index = (paddocks.firstIndex { $0.id == paddockID } ?? 0)+delta
        guard paddocks.indices.contains(index) else { return }
        paddockID = paddocks[index].id
    }
    private func paddockField(_ paddock:HerdPaddock,width:CGFloat) -> some View {
        LazyVGrid(columns:[GridItem(.adaptive(minimum:dynamicType.isAccessibilitySize ? width-32 : 156),spacing:6)],spacing:8) {
            ForEach(paddock.field) { horse in horseButton(horse,rail:false) }
        }.padding(.vertical,8)
    }

    private func horseCaption(name:String,status:String,hostName:String?,rail:Bool) -> some View {
        VStack(spacing:2) {
            Text(name).font(.system(.caption,design:.monospaced)).lineLimit(rail ? 1 : nil)
            Text(status).font(.caption.weight(.semibold))
            if let hostName { Text(hostName).font(.caption2).lineLimit(rail ? 1 : nil) }
        }.foregroundStyle(ranchTokens.inkColor).frame(maxWidth:.infinity).padding(.vertical,5)
            .ranchChromeSurface(ranchTokens, cornerRadius: 8)
    }

    private func horseButton(_ horse:HerdHorse,rail:Bool) -> some View {
        Button { select(horse) } label: {
            VStack(spacing:2) {
                ZStack(alignment:.bottom) {
                    Canvas { context,_ in
                        HerdArt().paint(&context,identity:horse.identity,
                                        pose:horse.pose(elapsed:elapsed,reduceMotion:reduced || !motionEnabled),
                                        gait:horse.gait(elapsed:elapsed,reduceMotion:reduced || !motionEnabled))
                    }
                    .frame(width:132,height:100)
                    .offset(x:horse.roam(elapsed:elapsed,enabled:motionEnabled && !dragging && !rail),
                            y:horse.state == .idle && !reduced && motionEnabled
                                ? sin(elapsed/4+horse.identity.phase)*0.5 : 0)
                    .saturation(horse.disconnected ? 0.25 : lighting.night ? 0.82 : 1)
                    .brightness(lighting.night ? -0.07 : 0)
                    if rail {
                        RanchFrontRail(night:lighting.night).offset(y:9)
                        Text("⚑").font(.title3).foregroundStyle(ranchColor(0xae5b61))
                            .frame(maxWidth:.infinity,maxHeight:.infinity,alignment:.trailing)
                    }
                }.frame(height:108).accessibilityHidden(true)
                horseCaption(name:horse.name,status:"\(herdMark(horse.state)) \(horse.statusText)",
                             hostName:horse.hostName,rail:rail)
            }.frame(width:rail ? railCardWidth : nil)
                .frame(minWidth:156,minHeight:44).contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(horse.disconnected)
        .accessibilityElement(children:.ignore)
        .accessibilityLabel("\(horse.name), \(horse.statusText), \(horse.agent.workspace.repo ?? "Other")\(horse.hostName.map { " on " + $0 } ?? "")")
        .accessibilityHint(horse.disconnected ? "Source disconnected" : "Opens recent output")
#if DEBUG
        .modifier(HerdRailFrameProbe(name:horse.name))
#endif
    }
}

func herdRepositoryCaption(_ paddock:HerdPaddock) -> String {
    let suffix = paddock.blockedCount > 0 ? " · \(paddock.blockedCount) at rail" : ""
    return "\(paddock.title) · \(paddock.field.count) here" + suffix
}

#if DEBUG
/// Geometry from the rendered production views, not a parallel layout model.
struct HerdRailFrames: PreferenceKey {
    static var defaultValue: [String: CGRect] { [:] }
    static func reduce(value: inout [String: CGRect], nextValue: () -> [String: CGRect]) {
        value.merge(nextValue(), uniquingKeysWith: { _, new in new })
    }
}

private struct HerdRailFrameProbe: ViewModifier {
    let name: String
    func body(content: Content) -> some View {
        content.background(GeometryReader { proxy in
            Color.clear.preference(key:HerdRailFrames.self,
                                   value:[name:proxy.frame(in:.named("herdRailLayout"))])
        })
    }
}
#endif

/// #456: full-screen cover mapping for the procedural ranch. The approved
/// native scene is a 390x640 world; the cover gives it ONE uniform scale
/// for both axes (never a per-axis stretch) and centers the overflow, which
/// the screen crops — the ranch paints behind every safe area without
/// deforming the approved horses or environment.
enum HerdRanchViewport {
    static let world = CGSize(width: 390, height: 640)
    static func scale(for size: CGSize) -> CGFloat {
        guard size.width > 0, size.height > 0 else { return 1 }
        return max(size.width / world.width, size.height / world.height)
    }
}

/// The cover container: the ranch content always receives a frame with the
/// world's exact aspect ratio, so its own canvas scales uniformly.
struct HerdRanchCover<Content: View>: View {
    @ViewBuilder var content: () -> Content
    var body: some View {
        GeometryReader { geometry in
            let scale = HerdRanchViewport.scale(for: geometry.size)
            content()
                .frame(width: HerdRanchViewport.world.width*scale,
                       height: HerdRanchViewport.world.height*scale)
                .frame(width: geometry.size.width, height: geometry.size.height)
                .clipped()
        }
    }
}

/// #456: native filter glyph (three decreasing lines) for the floating
/// scope control — the Herd renderer cannot use the platform picture APIs.
struct HerdFilterGlyph: View {
    let color: Color
    var body: some View {
        Canvas { context, size in
            for index in 0..<3 {
                let fraction = 1.0-Double(index)*0.33
                let y = size.height*(0.25+Double(index)*0.25)
                let rect = CGRect(x:0, y:y-size.height*0.09,
                                  width:size.width*fraction, height:size.height*0.18)
                context.fill(Path(roundedRect:rect, cornerRadius:size.height*0.09),
                             with:.color(color))
            }
        }
        .accessibilityHidden(true)
    }
}

/// #456: the Herd renderer stays fail-closed against the platform picture
/// APIs (ios/tools/herd-art/check-native-art.py), so the floating Settings
/// control draws its gear from native geometry like the rest of the surface.
struct HerdGearGlyph: View {
    let color: Color
    var body: some View {
        Canvas { context, size in
            let center = CGPoint(x: size.width/2, y: size.height/2)
            let radius = min(size.width, size.height)/2
            let hub = radius*0.62
            for index in 0..<8 {
                let angle = Double(index)/8*2*Double.pi
                var tooth = Path(roundedRect: CGRect(x: -radius*0.17, y: -radius,
                                                     width: radius*0.34, height: radius*0.55),
                                 cornerRadius: radius*0.09)
                tooth = tooth.applying(CGAffineTransform(rotationAngle: angle))
                tooth = tooth.applying(CGAffineTransform(translationX: center.x, y: center.y))
                context.fill(tooth, with: .color(color))
            }
            context.fill(Path(ellipseIn: CGRect(x: center.x-hub, y: center.y-hub,
                                                width: hub*2, height: hub*2)),
                         with: .color(color))
            context.blendMode = .destinationOut
            let bore = hub*0.42
            context.fill(Path(ellipseIn: CGRect(x: center.x-bore, y: center.y-bore,
                                                width: bore*2, height: bore*2)),
                         with: .color(.black))
        }
        .accessibilityHidden(true)
    }
}

#if DEBUG
extension HerdView {
    /// #456 recorded-evidence driver (launch-arg gated; Release never
    /// compiles it). Phases: Day full screen → Night full screen → next
    /// paddock → floating scope sheet → floating Settings sheet → long
    /// repository/horse names → empty scope. `-corralHerdOffline` records
    /// the disconnected/outage frame instead. The host capture script polls
    /// the Documents/ux-evidence markers and screenshots each phase.
    func runFullScreenEvidence() async {
        guard CommandLine.arguments.contains("-corral456FullScreenEvidence"),
              !evidenceFullScreenRan else { return }
        evidenceFullScreenRan = true
        if CommandLine.arguments.contains("-corralHerdOffline") {
            try? await Task.sleep(for:.seconds(2))
            EvidenceMarkers.write("456-offline-fullscreen")
            try? await Task.sleep(for:.seconds(9))
            return
        }
        evidenceEnvironment = .day
        try? await Task.sleep(for:.seconds(2))
        EvidenceMarkers.write("456-1-day-fullscreen")
        try? await Task.sleep(for:.seconds(9))
        evidenceEnvironment = .night
        try? await Task.sleep(for:.seconds(2))
        EvidenceMarkers.write("456-2-night-fullscreen")
        try? await Task.sleep(for:.seconds(9))
        evidenceEnvironment = .day
        movePage(1)
        try? await Task.sleep(for:.seconds(2))
        EvidenceMarkers.write("456-3-next-paddock")
        try? await Task.sleep(for:.seconds(9))
        movePage(-1)
        showFilters = true
        try? await Task.sleep(for:.seconds(2))
        EvidenceMarkers.write("456-4-scope-sheet")
        try? await Task.sleep(for:.seconds(9))
        showFilters = false
        try? await Task.sleep(for:.seconds(1))
        showSettings = true
        try? await Task.sleep(for:.seconds(2))
        EvidenceMarkers.write("456-5-settings-sheet")
        try? await Task.sleep(for:.seconds(9))
        showSettings = false
        try? await Task.sleep(for:.seconds(1))
        var longNames: [String: Agent] = [:]
        let states: [AgentState] = [.blocked, .blocked, .working, .working, .idle, .done, .unknown, .working]
        for i in 0..<8 {
            let id = "herdr:herd-fixture-long-\(i)"
            longNames[id] = Agent(agentId:id, state:states[i], seq:UInt64(i+1),
                                  ts:1_800_000_000_000, capabilities:["read_tail"],
                                  workspace:Workspace(repo:"extremely-long-repository-name-for-truncation-\(i)"),
                                  attachment:Attachment(kind:"herdr",reference:"fixture:456:\(i)"),
                                  displayName:"very-long-horse-display-name-for-truncation-\(i)")
        }
        HerdEvidence.model?.fleet.seedDemo(agents:longNames,rev:2)
        try? await Task.sleep(for:.seconds(2))
        EvidenceMarkers.write("456-6-long-names")
        try? await Task.sleep(for:.seconds(9))
        HerdEvidence.model?.fleet.seedDemo(agents:[:],rev:3)
        try? await Task.sleep(for:.seconds(2))
        EvidenceMarkers.write("456-7-empty-scope")
        try? await Task.sleep(for:.seconds(9))
    }

    /// #526 evidence: with the environment on AUTO, shift the lighting
    /// instant from local noon to local night — the resolved state must
    /// flip the RANCH and the CHROME (through the lighting report the ROOT
    /// pushes into the app palette) together, through the same path a real
    /// boundary crossing takes. Then park on the first and last paddock so
    /// the Previous/Next enabled-disabled pair is captured. Live in
    /// HerdView's own file because the pager index and the lighting instant
    /// it drives are HerdView state.
    func runPaletteAutoEvidence() async {
        guard CommandLine.arguments.contains("-corral526AutoEvidence"),
              !evidenceRan else { return }
        evidenceRan = true
        evidenceEnvironment = .auto
        theme.setHerdEnvironment(.auto)
        paddockID = paddocks.first?.id
        try? await Task.sleep(for:.seconds(2))
        EvidenceMarkers.write("526-16-pagination-first-page-previous-disabled")
        try? await Task.sleep(for:.seconds(9))
        paddockID = paddocks.last?.id
        try? await Task.sleep(for:.seconds(2))
        EvidenceMarkers.write("526-17-pagination-last-page-next-disabled")
        try? await Task.sleep(for:.seconds(9))
        evidenceClockOffset = Self.evidenceClockOffset(forLocalHour: 12)
        try? await Task.sleep(for:.seconds(2))
        EvidenceMarkers.write("526-18-auto-day")
        try? await Task.sleep(for:.seconds(9))
        evidenceClockOffset = Self.evidenceClockOffset(forLocalHour: 22)
        try? await Task.sleep(for:.seconds(2))
        EvidenceMarkers.write("526-19-auto-night-transition")
        try? await Task.sleep(for:.seconds(9))
    }

    /// The clock offset that lands the lighting instant on `hour` local
    /// time today (a fresh simulator has no location, so the ranch uses the
    /// fixed 07:00–19:00 window: noon resolves Day, 22:00 resolves Night).
    static func evidenceClockOffset(forLocalHour hour: Int) -> TimeInterval? {
        let calendar = Calendar.current
        guard let target = calendar.date(bySettingHour: hour, minute: 0,
                                         second: 0, of: Date()) else { return nil }
        return target.timeIntervalSince(Date())
    }
}
#endif

struct HerdScrollOffset: PreferenceKey {
    static var defaultValue: CGFloat = 0
    static func reduce(value:inout CGFloat,nextValue:() -> CGFloat) { value = nextValue() }
}
struct HerdScrollTracking: ViewModifier {
    @Binding var offset: CGFloat
    func body(content: Content) -> some View {
        if #available(iOS 18.0, *) {
            content.onScrollGeometryChange(for: CGFloat.self) { $0.contentOffset.x } action: { _, x in
                offset = max(0, x)
            }
        } else {
            content.onPreferenceChange(HerdScrollOffset.self) { offset = max(0, $0) }
        }
    }
}

func herdMark(_ state:AgentState) -> String {
    switch state {
    case .blocked: return "!"
    case .working: return "○"
    case .idle: return "◦"
    case .done: return "✓"
    case .unknown: return "?"
    }
}

/// #491: the summary bar's complete VoiceOver reading — full status names and
/// the exact scoped counts, in display order.
func herdCountsAccessibilityLabel(_ horses:[HerdHorse]) -> String {
    [AgentState.blocked,.working,.idle,.done,.unknown]
        .map { state in "\(horses.filter { $0.state == state }.count) \(state.rawValue)" }
        .joined(separator:", ")
}
