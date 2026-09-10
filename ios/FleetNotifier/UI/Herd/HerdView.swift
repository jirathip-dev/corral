import SwiftUI

struct HerdView: View {
    let horses: [HerdHorse]
    let obscured: Bool
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
    init(horses: [HerdHorse], obscured: Bool, select: @escaping (HerdHorse) -> Void,
         openBoard: @escaping () -> Void, retry: @escaping () async -> Void,
         clock: HerdClock? = nil) {
        self.horses = horses
        self.obscured = obscured
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
        VStack(spacing: 0) {
            summary
            if horses.contains(where: \.disconnected) { outage }
        GeometryReader { geometry in
            let maxScroll = CGFloat(max(0,paddocks.count-1))*geometry.size.width
            ZStack {
                RanchEnvironment(night:lighting.night,scroll:scroll,maxScroll:maxScroll,
                                 elapsed:elapsed,reduceMotion:!motionEnabled)
                VStack(spacing:0) {
                    Spacer(minLength:12).frame(maxHeight:88)
                    frontRail
                    if paddocks.isEmpty {
                        ContentUnavailableView("No agents in this scope",systemImage:"line.3.horizontal.decrease")
                    } else {
                        pager(width:geometry.size.width)
                        navigation
                    }
                }
            }
            .clipped()
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
        .task { await runHerdEvidence() }
        // Record from the current rendered value, not the task's captured
        // View struct (whose environment/immutable horse props may be stale).
        .onChange(of:evidencePhase) { _,phase in
            if let phase { HerdEvidence.record(phase,scene:self) }
        }
#endif
    }
    private var summary: some View {
        VStack(spacing:4) {
            ViewThatFits(in:.horizontal) {
                HStack(spacing:8) { counts }
                VStack(alignment:.leading,spacing:2) { counts }
            }
            Text(lighting.explanation).font(.caption2)
        }
        .foregroundStyle(theme.text)
        .padding(.vertical,6).frame(maxWidth:.infinity)
        .background(.ultraThinMaterial)
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
        }.foregroundStyle(theme.text).frame(maxWidth:.infinity).background(.regularMaterial)
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
    private var navigation: some View {
        let index = paddocks.firstIndex { $0.id == paddockID } ?? 0
        return HStack {
            Button("Previous") { movePage(-1) }.disabled(index == 0).frame(minWidth:44,minHeight:44)
            Spacer(minLength:4)
            Text("\(index+1) / \(paddocks.count)").font(.caption.monospacedDigit())
            Spacer(minLength:4)
            Button("Next") { movePage(1) }.disabled(index+1 >= paddocks.count).frame(minWidth:44,minHeight:44)
        }.font(.caption).padding(.horizontal,12).background(.regularMaterial)
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
                                        pose:horse.pose(elapsed:elapsed,reduceMotion:reduced || !motionEnabled))
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
