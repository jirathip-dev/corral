import SwiftUI

struct HerdView: View {
    let horses: [HerdHorse]
    let obscured: Bool
    /// #456: the board's reconciled scope labels (the SAME projections the
    /// board's Filters control uses) rendered by the floating top chrome.
    let scopeLabel: String
    let scopeSummary: String
    /// #456: the floating chrome drives the SAME sheets the board chrome
    /// does — the bindings are FleetView's own presentation state, so Herd
    /// never owns a parallel sheet or a duplicated toolbar.
    @Binding var showFilters: Bool
    @Binding var showSettings: Bool
    let select: (HerdHorse) -> Void
    let openBoard: () -> Void
    let retry: () async -> Void
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
    init(horses: [HerdHorse], obscured: Bool,
         scopeLabel: String = "Filters", scopeSummary: String = "All repositories",
         showFilters: Binding<Bool> = .constant(false),
         showSettings: Binding<Bool> = .constant(false),
         select: @escaping (HerdHorse) -> Void,
         openBoard: @escaping () -> Void, retry: @escaping () async -> Void,
         clock: HerdClock? = nil) {
        self.horses = horses
        self.obscured = obscured
        self.scopeLabel = scopeLabel
        self.scopeSummary = scopeSummary
        _showFilters = showFilters
        _showSettings = showSettings
        self.select = select
        self.openBoard = openBoard
        self.retry = retry
        _clock = StateObject(wrappedValue: clock ?? HerdClock())
    }
#if DEBUG
    @State var evidenceEnvironment: HerdEnvironmentChoice?
    @State var evidenceElapsed: Double?
    @State var evidenceReduceMotion = false
    @State var evidenceRan = false
    @State var evidencePhase: String?
    @State var evidenceFullScreenRan = false
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
    var lighting: HerdLighting {
        HerdSun.resolve(effectiveEnvironment,now:now,location:location.sample())
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
                if horses.contains(where: \.disconnected) { outage }
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
        .task {
            // #456 full-screen evidence supersedes the #459-era sequence in
            // the same launch (one deterministic marker stream).
            if !CommandLine.arguments.contains("-corral456FullScreenEvidence") {
                await runHerdEvidence()
            }
        }
        .task { await runFullScreenEvidence() }
        // Record from the current rendered value, not the task's captured
        // View struct (whose environment/immutable horse props may be stale).
        .onChange(of:evidencePhase) { _,phase in
            if let phase { HerdEvidence.record(phase,scene:self) }
        }
#endif
    }
    /// #456: the floating top chrome — scope + Settings on the first row,
    /// truthful scoped counts + the environment explanation beneath. Each
    /// surface floats over the ranch on the app's existing material (the
    /// ranch-context glass alignment is #457) with >= 44 pt targets and a
    /// safe-area-aware top margin.
    private var topChrome: some View {
        VStack(spacing:6) {
            HStack(spacing:8) {
                scopeControl
                settingsControl
            }
            statusSummary
        }
        .padding(.horizontal,12)
        .padding(.top,6)
    }
    /// The scope control repeats the board Filters control's contract over
    /// the ranch: same reconciled labels, same sheet, one >= 44 pt button.
    private var scopeControl: some View {
        Button { showFilters = true } label: {
            HStack(spacing:8) {
                HerdFilterGlyph(color: theme.accent)
                    .frame(width:16,height:16)
                VStack(alignment:.leading,spacing:1) {
                    Text(scopeLabel).font(.subheadline.weight(.semibold))
                        .foregroundStyle(theme.text).lineLimit(1)
                    Text(scopeSummary).font(.caption2).foregroundStyle(theme.subtext1)
                        .lineLimit(1).truncationMode(.tail)
                }
                Spacer(minLength:0)
            }
            .frame(maxWidth:.infinity,minHeight:44,alignment:.leading)
            .padding(.horizontal,12)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .background(.regularMaterial,in:RoundedRectangle(cornerRadius:15))
        .accessibilityElement(children:.ignore)
        .accessibilityLabel(scopeLabel + ", " + scopeSummary)
        .accessibilityHint("Opens host and repository filters")
    }
    /// #456: Settings stays reachable in Herd (the navigation toolbar is
    /// hidden here, so this floating gear is the mode's only gear — never a
    /// duplicated toolbar).
    private var settingsControl: some View {
        Button { showSettings = true } label: {
            HerdGearGlyph(color: theme.text)
                .frame(width:22,height:22)
                .frame(width:44,height:44)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .background(.regularMaterial,in:RoundedRectangle(cornerRadius:15))
        .accessibilityLabel("Settings")
        .accessibilityHint("Opens connection and notification settings")
    }
    /// Truthful scoped counts + environment note. When Dynamic Type or a
    /// narrow phone outgrows one line the counts wrap to a second/third row
    /// (the base summary's own fallback) instead of clipping.
    private var statusSummary: some View {
        VStack(spacing:4) {
            ViewThatFits(in:.horizontal) {
                HStack(spacing:8) { counts }
                VStack(alignment:.leading,spacing:2) { counts }
            }
            Text(lighting.explanation).font(.caption2)
        }
        .foregroundStyle(theme.text)
        .padding(.vertical,6).padding(.horizontal,12)
        .frame(maxWidth:.infinity)
        .background(.regularMaterial,in:RoundedRectangle(cornerRadius:15))
    }
    @ViewBuilder private var counts: some View {
        ForEach([AgentState.blocked,.working,.idle,.done,.unknown],id:\.self) { state in
            Text("\(herdMark(state)) \(horses.filter { $0.state == state }.count) \(state.rawValue)")
                .font(.system(.caption2,design:.monospaced))
        }
    }
    private var outage: some View {
        VStack(spacing:2) {
            Text("Source disconnected · last-known agents").font(.caption.weight(.semibold))
            Text("Unknown · blocked status cannot be confirmed").font(.caption2)
            HStack {
                Button("Open Board",action:openBoard).frame(minWidth:44,minHeight:44)
                Button("Retry") { Task { await retry() } }.frame(minWidth:44,minHeight:44)
            }
        }.foregroundStyle(theme.text)
            .padding(.vertical,6).padding(.horizontal,12).frame(maxWidth:.infinity)
            .background(.regularMaterial,in:RoundedRectangle(cornerRadius:15))
            .padding(.horizontal,12).padding(.top,6)
    }
    private var frontRail: some View {
        VStack(alignment:.leading,spacing:4) {
            Text("! FRONT RAIL · \(rail.count)\(rail.contains(where:\.disconnected) ? " LAST KNOWN" : " BLOCKED")")
                .font(.caption.weight(.semibold)).foregroundStyle(theme.text)
                .padding(6).background(.regularMaterial,in:RoundedRectangle(cornerRadius:6)).padding(.leading,12)
            if !rail.isEmpty {
                ScrollView(.horizontal) {
                    LazyHStack(alignment:.top,spacing:10) {
                        ForEach(rail) { horse in horseButton(horse,rail:true) }
                    }.padding(.horizontal,12)
                }.scrollIndicators(.hidden)
            } else {
                RanchFrontRail(night:lighting.night)
            }
        }
        .accessibilityElement(children:.contain).accessibilityLabel("Global blocked front rail")
    }
    /// #456-r1: the rail + paddock column. At accessibility sizes a small
    /// phone cannot show the rail, the paddock header and the field at once,
    /// so the column scrolls under the pinned chrome and above the pinned
    /// navigation instead of being squeezed (the squeeze previously drew the
    /// rail outside its pills and pushed the navigation out of place). At the
    /// approved normal sizes the composition is unchanged.
    @ViewBuilder private func herdColumn(width:CGFloat) -> some View {
        if dynamicType.isAccessibilitySize {
            ScrollView(.vertical) {
                VStack(spacing:0) {
                    frontRail
                    pager(width:width)
                }
            }
        } else {
            VStack(spacing:0) {
                Spacer(minLength:12).frame(maxHeight:88)
                frontRail
                pager(width:width)
            }
        }
    }
    private func pager(width:CGFloat) -> some View {
        ScrollView(.horizontal) {
            LazyHStack(spacing:0) {
                ForEach(paddocks) { paddock in
                    VStack(spacing:4) {
                        HStack {
                            Text(paddock.title).font(.headline).lineLimit(1)
                            Spacer(minLength:4)
                            Text("\(paddock.field.count) here · \(paddock.blockedCount) at rail").font(.caption2)
                        }.foregroundStyle(theme.text).padding(8)
                            .background(.regularMaterial,in:RoundedRectangle(cornerRadius:8))
                        ScrollView(.vertical) {
                            LazyVGrid(columns:[GridItem(.adaptive(minimum:dynamicType.isAccessibilitySize ? width-32 : 156),spacing:6)],spacing:8) {
                                ForEach(paddock.field) { horse in horseButton(horse,rail:false) }
                            }.padding(.vertical,8)
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
    /// Previous / position / Next as a rounded material pill instead of the
    /// old opaque full-width bar.
    private var navigation: some View {
        let index = paddocks.firstIndex { $0.id == paddockID } ?? 0
        return HStack {
            Button("Previous") { movePage(-1) }.disabled(index == 0).frame(minWidth:44,minHeight:44)
            Spacer(minLength:4)
            Text("\(index+1) / \(paddocks.count)").font(.caption.monospacedDigit())
            Spacer(minLength:4)
            Button("Next") { movePage(1) }.disabled(index+1 >= paddocks.count).frame(minWidth:44,minHeight:44)
        }.font(.caption)
            .padding(.horizontal,12).padding(.vertical,6)
            .background(.regularMaterial,in:RoundedRectangle(cornerRadius:15))
            .padding(.horizontal,12).padding(.bottom,6)
            .accessibilityLabel("Repository paddocks")
    }
    func movePage(_ delta:Int) {
        let index = (paddocks.firstIndex { $0.id == paddockID } ?? 0)+delta
        guard paddocks.indices.contains(index) else { return }
        paddockID = paddocks[index].id
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
                VStack(spacing:2) {
                    Text(horse.name).font(.system(.caption,design:.monospaced))
                    Text("\(herdMark(horse.state)) \(horse.statusText)").font(.caption.weight(.semibold))
                    if let hostName = horse.hostName { Text(hostName).font(.caption2) }
                }.foregroundStyle(theme.text).frame(maxWidth:.infinity).padding(.vertical,5)
                    .background(.regularMaterial,in:RoundedRectangle(cornerRadius:8))
            }.frame(width:rail ? (dynamicType.isAccessibilitySize ? 240 : 164) : nil)
                .frame(minWidth:156,minHeight:44).contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(horse.disconnected)
        .accessibilityElement(children:.ignore)
        .accessibilityLabel("\(horse.name), \(horse.statusText), \(horse.agent.workspace.repo ?? "Other")\(horse.hostName.map { " on " + $0 } ?? "")")
        .accessibilityHint(horse.disconnected ? "Source disconnected" : "Opens recent output")
    }
}

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
