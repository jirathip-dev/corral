import SwiftUI

struct RanchEnvironment: View {
    let night: Bool
    let scroll: CGFloat
    let maxScroll: CGFloat
    let elapsed: Double
    let reduceMotion: Bool
    // #459: ambient motion also falls back to the intentional static scene
    // under Low Power or a serious/critical thermal state. These are read
    // once and re-evaluated when iOS posts the process notifications; no
    // timer or task is created here — the scene clock stays the only
    // timing owner.
    @State private var lowPower = ProcessInfo.processInfo.isLowPowerModeEnabled
    @State private var thermalState = ProcessInfo.processInfo.thermalState
    static let planes: [RanchPlane] = [.sky,.hills,.ground,.barnTrees,.rearFences,.foreground]

    private var ambientFrozen: Bool {
        reduceMotion || !HerdAmbientPolicy.ambientMotionAllowed(lowPower:lowPower,thermal:thermalState)
    }
    var body: some View {
        ZStack {
            ForEach(Self.planes) { plane in
                Canvas { context, size in
                    let offset = plane.offset(scroll:scroll,coverage:maxScroll,reduceMotion:reduceMotion)
                    let painter = RanchPainter(night:night,elapsed:ambientFrozen ? 0 : elapsed)
                    context.scaleBy(x:size.width/390,y:size.height/640)
                    context.translateBy(x:offset*390/size.width,y:0)
                    painter.draw(plane,&context,left:-offset*390/size.width)
                }
                .accessibilityHidden(true)
            }
        }
        .onReceive(NotificationCenter.default.publisher(for:.NSProcessInfoPowerStateDidChange)) { _ in
            refreshAmbientFallback()
        }
        .onReceive(NotificationCenter.default.publisher(for:ProcessInfo.thermalStateDidChangeNotification)) { _ in
            refreshAmbientFallback()
        }
        .clipped()
        .allowsHitTesting(false)
    }
    private func refreshAmbientFallback() {
        lowPower = ProcessInfo.processInfo.isLowPowerModeEnabled
        thermalState = ProcessInfo.processInfo.thermalState
    }
}

// Every visible layer is native geometry. Noise is indexed, never random per
// frame. Each plane has its own world origin and contiguous coverage.
//
// #460: the sky's ambient motion is bounded internal math on the ONE shared
// scene clock — no second timer, no per-frame randomness. Every term below
// is a pure function of `elapsed`, so a sampled phase is deterministic, the
// same cloud/star identity is re-drawn each frame, and the composition at
// elapsed 0 is exactly the approved static sky.
enum RanchSky {
    /// Cloud drift in scene points per second: gentle, but clearly
    /// perceptible at phone scale (≈29 pt — 7% of the 390 pt world — every
    /// 12 s of real time).
    static let cloudSpeed = 2.4
    /// Star-field / Milky Way drift in scene points per second: slower and
    /// coarser than the weather, the way the night sky turns. Both the
    /// stars and the band ride this ONE rate, so the night sky moves as one
    /// coherent field and stays distinct from the cloud drift.
    static let celestialSpeed = 0.7

    static func cloudDrift(_ elapsed: Double) -> Double { elapsed * cloudSpeed }
    static func celestialDrift(_ elapsed: Double) -> Double { elapsed * celestialSpeed }

    /// The Milky Way copies that can reach a canvas whose visible world
    /// window is [left, left+390] under the celestial drift. Each copy's
    /// visible ink spans [centre-39, centre+382] scene points: the hill
    /// ridge occludes the band below its silhouette and the world top clips
    /// its thin tip, so copies enter and leave where the band is already
    /// hidden — no popping and no wrap seam. Copies further out cannot
    /// touch the canvas and are never drawn, so the blurred strokes are not
    /// duplicated gratuitously.
    static func bandCopyRange(elapsed: Double, left: CGFloat) -> ClosedRange<Int> {
        let centre = celestialDrift(elapsed)
        let edge = Double(left)
        let first = ((edge - 382 - centre) / 390).rounded(.up)
        let last = ((edge + 429 - centre) / 390).rounded(.down)
        return Int(first)...Int(last)
    }
}

struct RanchPainter {
    let night: Bool
    let elapsed: Double
    func color(_ day:Int,_ dark:Int) -> Color { ranchColor(night ? dark : day) }
    func noise(_ i:Int,_ salt:Int = 0) -> Double {
        let n = UInt64(truncatingIfNeeded: i &* 1664525 &+ salt &* 1013904223)
        return Double((n ^ (n >> 13)) &* 1274126177 % 65536)/65535
    }
    func fill(_ c:inout GraphicsContext,_ path:Path,_ day:Int,_ dark:Int,opacity:Double = 1) {
        c.fill(path,with:.color(color(day,dark).opacity(opacity)))
    }
    func ellipse(_ x:Double,_ y:Double,_ rx:Double,_ ry:Double) -> Path {
        Path(ellipseIn:CGRect(x:x-rx,y:y-ry,width:rx*2,height:ry*2))
    }
    func draw(_ plane:RanchPlane,_ c:inout GraphicsContext,left:CGFloat) {
        switch plane {
        case .sky: sky(&c,left:left)
        case .hills: hills(&c,left:left)
        case .ground: ground(&c,left:left)
        case .barnTrees: structures(&c,left:left)
        case .rearFences: fence(&c,left:left,y:256)
        case .foreground: grass(&c,left:left)
        case .frontRail: fence(&c,left:left,y:15)
        case .horses: break
        }
    }
    func sky(_ c:inout GraphicsContext,left:CGFloat) {
        c.fill(Path(CGRect(x:left,y:0,width:390,height:640)),with:.linearGradient(
            Gradient(colors:[color(0x388fc4,0x121a32),color(0xe5e8c9,0x667991)]),
            startPoint:CGPoint(x:0,y:0),endPoint:CGPoint(x:0,y:640)))
        if night {
            // Wide, oblique Milky Way with feathered native gradients and
            // stable dust lanes. Not a stripe of randomly flickering pixels.
            // #460: the band rides the shared celestial drift; its periodic
            // copies re-enter exactly where the hills and the world top
            // already occlude it, so the wrap never seams or pops.
            let celestial = RanchSky.celestialDrift(elapsed)
            for band in 0..<6 {
                var galaxy = c
                galaxy.addFilter(.blur(radius:Double(14-band)))
                for copy in RanchSky.bandCopyRange(elapsed:elapsed,left:left) {
                    let base = Double(copy)*390 + celestial
                    let p = horsePath([.m(base-60,220),.q(base+60,74,base+220,3),
                                       .q(base+275,-20,base+335,-32)])
                    galaxy.stroke(p,with:.color(ranchColor(0xbab8db).opacity(0.022)),
                                  style:StrokeStyle(lineWidth:CGFloat(66-band*8),lineCap:.round))
                }
            }
            // #460: elapsed moves the star field's position; it never
            // re-rolls a star's seeded identity. The scan range follows the
            // drift so coverage stays continuous at any scroll offset.
            for i in Int((Double(left)-celestial)/3)-2...Int((Double(left)+390-celestial)/3)+2 {
                let x = Double(i)*3 + noise(i,8)*3 + celestial
                let y = noise(i,3)*225
                let radius = 0.35 + noise(i,7)*0.7
                let brightness = 0.3 + 0.45*noise(i,9) + 0.08*sin(elapsed/3+Double(i))
                fill(&c,ellipse(x,y,radius,radius),0xffffff,0xe7e5f4,opacity:brightness)
            }
            // The moon is a fixed landmark: it keeps the approved position.
            fill(&c,ellipse(306,56,16,16),0xffffff,0xd3dfeb,opacity:0.84)
            fill(&c,ellipse(301,51,15,15),0xffffff,0x1d2841)
        }
        // #460: clouds drift on the shared clock (day and night alike); the
        // tile range follows the drift, so the field wraps continuously
        // without a seam or a blank edge at phone widths.
        let cloudDrift = RanchSky.cloudDrift(elapsed)
        for i in Int((Double(left)-cloudDrift)/130)-1...Int((Double(left)+390-cloudDrift)/130)+1 {
            let x = Double(i)*130 + 45 + cloudDrift + sin(elapsed/20+Double(i))*1.5
            let y = 36 + noise(i,30)*86
            var cloud = c
            cloud.addFilter(.blur(radius:4))
            for j in 0..<5 {
                fill(&cloud,ellipse(x+Double(j)*12,y-noise(j,i)*9,18,5+noise(j,2)*5),
                     0xe3f1e9,0xa6b6c6,opacity:night ? 0.055 : 0.20)
            }
        }
    }
    func hills(_ c:inout GraphicsContext,left:CGFloat) {
        for ridge in 0..<4 {
            let spacing = 35.0
            var p = Path(); p.move(to:CGPoint(x:left-50,y:640))
            for i in Int((left-50)/spacing)-1...Int((left+440)/spacing)+1 {
                let y = 147 + Double(ridge)*30 + noise(i,ridge+17)*36
                p.addLine(to:CGPoint(x:Double(i)*spacing,y:y))
            }
            p.addLine(to:CGPoint(x:left+480,y:640)); p.closeSubpath()
            let day = [0xa9c9bf,0x9fb9a5,0x8aab88,0x79966d][ridge]
            let dark = [0x617484,0x586c78,0x4c666a,0x425b5a][ridge]
            fill(&c,p,day,dark)
        }
    }
    func ground(_ c:inout GraphicsContext,left:CGFloat) {
        let p = horsePath([.m(left-20,258),.q(left+165,228,left+410,268),
                           .l(left+410,640),.l(left-20,640),.close])
        c.fill(p,with:.linearGradient(Gradient(colors:[color(0xbcc17b,0x63746a),
            color(0x8e9c58,0x415d55),color(0x485e3b,0x273f3e)]),
            startPoint:CGPoint(x:0,y:250),endPoint:CGPoint(x:0,y:640)))
        for i in Int(left/160)-1...Int((left+390)/160)+1 {
            let x = Double(i)*160
            fill(&c,horsePath([.m(x+130,258),.q(x+40,430,x+180,640),.l(x+226,640),
                              .q(x+62,438,x+145,258),.close]),0xd2c48c,0x789085,opacity:0.18)
            fill(&c,horsePath([.m(x,277),.q(x-35,395,x+130,520),.l(x+55,566),
                              .q(x-55,389,x-30,290),.close]),0x334c2e,0x172e32,opacity:0.12)
        }
        // Fixed small stipples supply the R2 grass/material grain without
        // decoding exported paint. Indices are world-space, not screen-space.
        for i in Int(left/5)*19...Int((left+395)/5)*19 {
            let column = i/19
            let x = Double(column)*5 + noise(i,8)*5
            let y = 270 + noise(i,19)*370
            fill(&c,ellipse(x,y,0.3+noise(i,2)*0.7,0.3),0xe8e3b3,0xb9c3b1,opacity:0.13)
        }
    }
    func structures(_ c:inout GraphicsContext,left:CGFloat) {
        for tile in Int(left/390)-1...Int((left+390)/390)+1 {
            let x = Double(tile)*390
            // Barn, door, split trim and warm side wall.
            fill(&c,horsePath([.m(x+153,243),.l(x+180,219),.l(x+210,242),.l(x+210,279),
                              .l(x+153,279),.close]),0xa86e52,0x705b58)
            fill(&c,horsePath([.m(x+148,245),.l(x+180,215),.l(x+216,243),.l(x+209,246),
                              .l(x+180,224),.l(x+155,248),.close]),0x536456,0x344b50)
            fill(&c,Path(CGRect(x:x+175,y:252,width:14,height:27)),0x344635,0x182d35)
            fill(&c,Path(CGRect(x:x+175,y:235,width:9,height:9)),0xe5dfb8,0xceb581,opacity:0.8)
            for t in [(x+7,225.0,1.0),(x+365,228,1.1),(x+238,258,0.5),(x+326,262,0.38)] {
                tree(&c,x:t.0,y:t.1,scale:t.2)
            }
        }
    }
    func tree(_ c:inout GraphicsContext,x:Double,y:Double,scale:Double) {
        // #459: the trunk stays grounded and unswayed; only the leaf
        // clusters ride the shared wind phase.
        fill(&c,Path(CGRect(x:x-2*scale,y:y-8*scale,width:4*scale,height:45*scale)),0x776745,0x3c4c4a)
        let root = RanchWind.anchor(x)
        for i in 0..<22 {
            let dx = (noise(i,20)-0.5)*48*scale
            let dy = (noise(i,7)-0.5)*35*scale
            // Cluster phase plus a height gradient: upper clusters travel
            // further than lower ones, so the canopy shears gently instead
            // of translating as one rigid sticker.
            let cluster = Double(i % 4) * 0.5 + noise(i,11) * 0.7
            let lift = 1.1 - (dy / (35 * scale)) * 0.35
            let sway = RanchWind.sway(elapsed:elapsed,anchor:root + cluster,
                                      amplitude:RanchWind.canopyAmplitude * scale * lift)
            let bob = RanchWind.sway(elapsed:elapsed,anchor:root + cluster + 1.9,
                                     amplitude:0.5 * scale * lift)
            fill(&c,ellipse(x+dx+sway,y+dy+bob,9*scale,11*scale),
                 i%3 == 0 ? 0x859b5c : 0x66874f,i%3 == 0 ? 0x557068 : 0x405e59,opacity:0.9)
            fill(&c,ellipse(x+dx+sway-2*scale,y+dy+bob-3*scale,5*scale,4*scale),0xb0b67b,0x96aea0,opacity:0.10)
        }
    }
    func fence(_ c:inout GraphicsContext,left:CGFloat,y:Double) {
        for i in Int(left/52)-1...Int((left+390)/52)+1 {
            let x = Double(i)*52
            fill(&c,Path(roundedRect:CGRect(x:x,y:y-7,width:6,height:44),cornerRadius:1),0x9c916a,0x73847f)
            fill(&c,Path(CGRect(x:x+1,y:y-7,width:1.4,height:44)),0xe2d7ac,0xd0dbd4,opacity:0.5)
        }
        for dy in [4.0,22] {
            fill(&c,Path(CGRect(x:left-15,y:y+dy,width:420,height:5)),0x8b825d,0x6b7d75)
            fill(&c,Path(CGRect(x:left-15,y:y+dy,width:420,height:1.3)),0xd6c99f,0xc4d6e0,opacity:0.8)
        }
    }
    func grass(_ c:inout GraphicsContext,left:CGFloat) {
        for i in Int(left/9)-1...Int((left+390)/9)+1 {
            let x = Double(i)*9
            let y = 303 + noise(i,43)*333
            let bend = RanchWind.grassBend(elapsed:elapsed,worldX:x)
            for blade in 0..<3 {
                let dx = Double(blade)*2
                let p = horsePath([.m(x+dx,y),.q(x+dx-1+bend,y-5,x+dx-3+bend,y-8-noise(i,4)*4)])
                c.stroke(p,with:.color(color(0xaeb57a,0x90a397).opacity(0.25)),lineWidth:0.8)
            }
        }
        // Sun / moon direction changes native cast-light paths, not horse identity.
        let wash = Path(CGRect(x:left,y:0,width:390,height:640))
        c.fill(wash,with:.linearGradient(Gradient(colors:[color(0xffeeab,0xbbcfe4).opacity(0.03),.clear]),
            startPoint:CGPoint(x:night ? left+390 : left,y:0),endPoint:CGPoint(x:left+195,y:500)))
    }
}

struct RanchFrontRail: View {
    let night: Bool
    var body: some View {
        Canvas { context,size in
            context.scaleBy(x:size.width/390,y:size.height/44)
            RanchPainter(night:night,elapsed:0).draw(.frontRail,&context,left:0)
        }
        .frame(height:44)
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }
}
