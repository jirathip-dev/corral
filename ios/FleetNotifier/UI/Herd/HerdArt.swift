import SwiftUI

// Native vector operations, authored from #442 ORIGINAL V1 horsesvg.py.
// No bitmap, SVG decoder, file, asset catalog, or network art path.
enum HorseLine {
    case m(CGFloat, CGFloat), l(CGFloat, CGFloat)
    case q(CGFloat, CGFloat, CGFloat, CGFloat)
    case c(CGFloat, CGFloat, CGFloat, CGFloat, CGFloat, CGFloat), close
}

struct HorseInk {
    var path: Path
    let color: String
    var opacity: Double = 1
    var stroke: CGFloat = 0
    var part: String = ""
}

func horsePath(_ lines: [HorseLine]) -> Path {
    var p = Path()
    for line in lines {
        switch line {
        case let .m(x,y): p.move(to: CGPoint(x:x,y:y))
        case let .l(x,y): p.addLine(to: CGPoint(x:x,y:y))
        case let .q(x,y,u,v): p.addQuadCurve(to: CGPoint(x:u,y:v), control: CGPoint(x:x,y:y))
        case let .c(x,y,u,v,a,b):
            p.addCurve(to: CGPoint(x:a,y:b), control1: CGPoint(x:x,y:y), control2: CGPoint(x:u,y:v))
        case .close: p.closeSubpath()
        }
    }
    return p
}

struct HerdArt {
    static func load() throws -> HerdArt { HerdArt() }
    static let coats = [0x8a5a33,0xa05c2c,0x3b3b44,0xb9bcc6,0xc89a56,0xb08d5e,0x96685a,0xbd8f4e]
    static let darks = [0x6f4527,0x7d4520,0x2a2a31,0x989ca9,0xa37a3c,0x8e6f45,0x77524a,0x96703a]
    static let manes = [0x2e2019,0x5d3317,0x17161a,0x7d8089,0xe8dcc0,0x41321f,0x4a342c,0x241a10]

    /// #448: `gait` swings the four legs in two diagonal pairs around the
    /// approved rest angles — legs 0 (front) + 3 (rear) step together, legs
    /// 1 (rear) + 2 (front) oppose them — and each hoof ink rides its leg's
    /// arc. The default standstill leaves every approved pose byte-identical.
    func drawing(_ identity: HorseIdentity, pose: HorsePose, gait: HorseGait = .standstill) -> [HorseInk] {
        var art = HorseDraft(identity: identity)
        let belly = [66.0,69,72][identity.breed]
        let width = [5.0,6.2,7.4][identity.breed]
        let height = [34.0,31,28][identity.breed]
        let raised = pose == .blocked || pose == .alertStatic || pose == .working
        let rest: [Double] = pose == .working ? [28,-26,-26,24]
            : pose == .blocked ? [-26,0,0,0] : pose == .unknown ? [6,-6,-6,6] : [0,0,0,0]
        let step = gait.isStepping && (pose == .working || pose == .stand) ? gait.swing * sin(2 * .pi * gait.phase) : 0
        let legs = [rest[0] + step, rest[1] - step, rest[2] - step, rest[3] + step]
        art.ellipse(64,97,44,4,"2f2a26", opacity: 0.14)
        let bodyStart = art.inks.count
        art.leg(78,belly-6,width*0.88,height,legs[1],"dark",part:"leg-1")
        art.leg(51,belly-6,width*0.88,height,legs[3],"dark",part:"leg-3")
        art.add(raised ? [.m(40,41),.q(30,35,26,21)] : [.m(39,43),.q(28,53,27,69)], "mane", stroke: 6.5)
        if pose == .graze { art.grazing() }
        else {
            let start = art.inks.count
            art.upright()
            let angle: Double = pose == .working ? -8 : pose == .blocked || pose == .alertStatic ? -16
                : pose == .done ? 12 : pose == .unknown ? 4 : -6
            art.rotate(from: start, angle: angle, x:90,y:46)
        }
        art.add([.m(40,40),.q(47,35.5,60,38.5),.q(72,41,80,36),.q(87,33,90,42),
                 .q(92,48,92,54),.q(92,belly-6,88,belly-2),.q(66,belly+2,44,belly-4),
                 .q(31,belly-9,30,54),.q(30,46,34,42),.q(37,39.5,40,40),.close],"body")
        art.add([.m(34,42),.q(30,46,30,54),.q(31,belly-9,44,belly-4),.q(40,belly-5,38,46),
                 .q(37,42,38,40),.close],"dark",opacity:0.32)
        art.add([.m(80,36),.q(87,33,90,42),.q(91,47,91,52),.q(85,50,82,44),
                 .q(80,40,80,36),.close],"dark",opacity:0.22)
        if identity.tack == 0 {
            art.add([.m(50,37),.q(60,33,70,36),.l(71,belly-22),.q(61,belly-17,51,belly-21),.close],"7a4f2c")
            art.add([.m(60,belly-19),.q(60,belly-6,64,belly-3)],"5d3a1f",stroke:1.8)
        } else if identity.tack == 1 { art.rect(52,belly-30,20,9,3,"c2543f") }
        art.leg(84,belly-6,width,height,legs[0],"body",part:"leg-0")
        art.leg(43,belly-6,width,height,legs[2],"body",part:"leg-2")
        if identity.breed == 2 {
            for (x, angle) in [(84.0, step), (43, -step)] {
                let start = art.inks.count
                art.rect(x-width/2-1,belly-6+height-8,width+2,6,3,"mane")
                if angle != 0 { art.rotate(from:start,angle:angle,x:x,y:belly-6) }
            }
        }
        if pose == .graze {
            // Grazing-only joint accents: original barrel, leg lengths and hoof anchors retained.
            for x in [84.0,43] { art.ellipse(x,belly+9,width/2,1.6,"dark",opacity:0.55) }
        }
        if pose == .working {
            art.rotate(from: bodyStart, angle:-4, x:66,y:52)
            for index in bodyStart..<art.inks.count {
                art.inks[index].path = art.inks[index].path.offsetBy(dx:0,dy:-1.5)
            }
        }
        return art.inks
    }

    func paint(_ context: inout GraphicsContext, identity: HorseIdentity, pose: HorsePose, gait: HorseGait = .standstill) {
        for ink in drawing(identity,pose:pose,gait:gait) {
            let value: Int
            switch ink.color {
            case "body": value = Self.coats[identity.coat]
            case "dark": value = Self.darks[identity.coat]
            case "mane": value = Self.manes[identity.coat]
            default: value = Int(ink.color, radix:16) ?? 0x2f2a26
            }
            let color = ranchColor(value).opacity(ink.opacity)
            if ink.stroke > 0 {
                context.stroke(ink.path, with:.color(color), style:StrokeStyle(lineWidth:ink.stroke,lineCap:.round))
            } else { context.fill(ink.path, with:.color(color)) }
        }
        if pose == .unknown {
            context.fill(Path(ellipseIn:CGRect(x:57.5,y:0.5,width:17,height:17)), with:.color(ranchColor(0xefe7dc)))
            context.stroke(Path(ellipseIn:CGRect(x:57.5,y:0.5,width:17,height:17)), with:.color(ranchColor(0x2f2a26)), lineWidth:1.2)
            context.draw(Text("?").font(.system(size:12.5,weight:.bold)).foregroundColor(ranchColor(0x2f2a26)),
                         at:CGPoint(x:66,y:9))
        }
    }
}

struct HorseDraft {
    let identity: HorseIdentity
    var inks: [HorseInk] = []
    mutating func add(_ p:[HorseLine],_ color:String,opacity:Double = 1,stroke:CGFloat = 0,part:String = "") {
        inks.append(HorseInk(path:horsePath(p),color:color,opacity:opacity,stroke:stroke,part:part))
    }
    mutating func ellipse(_ x:Double,_ y:Double,_ rx:Double,_ ry:Double,_ color:String,opacity:Double = 1) {
        inks.append(HorseInk(path:Path(ellipseIn:CGRect(x:x-rx,y:y-ry,width:2*rx,height:2*ry)),color:color,opacity:opacity))
    }
    mutating func rect(_ x:Double,_ y:Double,_ w:Double,_ h:Double,_ r:Double,_ color:String) {
        inks.append(HorseInk(path:Path(roundedRect:CGRect(x:x,y:y,width:w,height:h),cornerRadius:r),color:color))
    }
    mutating func rotate(from start:Int,angle:Double,x:Double,y:Double) {
        let transform = CGAffineTransform(translationX:x,y:y).rotated(by:angle * .pi/180).translatedBy(x:-x,y:-y)
        for index in start..<inks.count { inks[index].path = inks[index].path.applying(transform) }
    }
    mutating func leg(_ x:Double,_ y:Double,_ w:Double,_ h:Double,_ a:Double,_ color:String,part:String = "") {
        let start = inks.count
        rect(x-w/2,y,w,h,w/2,color)
        rect(x-w/2,y+h-5,w,5,2.5,"3a332e")
        if !part.isEmpty {
            inks[start].part = part
            inks[start+1].part = part + "-hoof"
        }
        rotate(from:start,angle:a,x:x,y:y)
    }
    mutating func hat(_ x:Double,_ y:Double,_ r:Double) {
        ellipse(x,y,r+6.5,4,"d8b46a")
        add([.m(x-r,y),.c(x-r,y-r*0.5523,x-r*0.5523,y-r,x,y-r),
             .c(x+r*0.5523,y-r,x+r,y-r*0.5523,x+r,y),.close],"d8b46a")
        add([.m(x-r,y),.c(x-r,y-r*0.5523,x-r*0.5523,y-r,x,y-r),
             .c(x+r*0.5523,y-r,x+r,y-r*0.5523,x+r,y),.close],"8a5a33",stroke:r == 6 ? 1.4 : 1.3)
    }
    mutating func upright() {
        add([.m(80,40),.c(92,30,102,22,110,15),.l(118,20),.q(124,23,127,30),.q(129,34,127,36),
             .q(124,39,120,38),.q(112,36,106,31),.q(100,40,96,50),.l(84,52),.close],"body")
        add([.m(104,28),.q(110,32,116,34),.q(108,38,101,33),.close],"dark",opacity:0.3)
        add([.m(122,27),.q(128,31,127,36),.q(124,39,120,38),.q(120,32,122,27),.close],"c9958b")
        ellipse(124.5,32.5,1.1,1.1,"2f2a26")
        add([.m(109,14),.l(111,5),.l(116,11),.close],"body")
        add([.m(111,11),.l(112,8),.l(114,10),.close],"c9958b")
        ellipse(116,22.5,1.7,1.7,"1d1a17")
        if identity.blaze { add([.m(119,21),.q(123,25,125,30),.l(123,31),.q(120,26,117,22),.close],"efe7dc") }
        if identity.accessory == 1 { hat(115,11,6) }
        switch identity.mane {
        case 0:
            add([.m(111,13),.q(101,16,95,24),.q(90,31,84,35),.l(78,36),.q(87,28,93,19),
                 .q(100,10,111,13),.close],"mane")
            add([.m(111,13),.q(116,15,118,20),.q(113,22,110,18),.close],"mane")
        case 1:
            for (x,y) in [(91.0,29.0),(98,23),(105,17)] { ellipse(x,y,3.4,3.4,"mane") }
        default:
            add([.m(82,37),.q(95,26,108,15),.l(111,18),.q(98,28,86,39),.close],"mane")
        }
        if identity.accessory == 0 { add([.m(84,38),.l(97,40),.l(90,49),.close],"c2543f") }
    }
    mutating func grazing() {
        // Shoulder/chest wedge supports a descending crest. Head is separate
        // from the neck: poll → forehead → blunt muzzle → cheek/jaw notch.
        add([.m(74,40),.q(91,34,102,48),.q(109,55,111,64),.l(104,75),
             .q(98,66,92,60),.q(86,55,80,53),.close],"body",part:"grazing-neck")
        add([.m(110,61),.q(116,62,120,75),.l(125,90),.q(125,95,119,96),
             .l(114,91),.l(109,82),.q(101,78,104,71),.close],"body",part:"grazing-head")
        add([.m(108,64),.l(107,54),.l(113,60),.close],"body",part:"grazing-ear")
        add([.m(115,66),.l(118,57),.l(119,69),.close],"body",part:"grazing-ear")
        add([.m(109,62),.l(108,57),.l(111,61),.close],"c9958b")
        add([.m(104,74),.q(108,82,114,81),.l(110,85),.q(103,80,104,74),.close],"dark",opacity:0.35)
        add([.m(118,86),.l(124,87),.l(125,91),.q(125,95,119,96),.l(115,91),.close],"c9958b")
        ellipse(121.5,91,1,1,"2f2a26")
        ellipse(114.5,71,1.6,1.6,"1d1a17")
        if identity.blaze { add([.m(117,72),.l(121,86),.l(119,88),.l(115,74),.close],"efe7dc") }
        if identity.accessory == 1 { hat(110,58,5.5) }
        switch identity.mane {
        case 0:
            add([.m(78,36),.q(95,37,103,50),.q(108,57,108,65),.l(103,68),
                 .q(102,55,96,50),.q(88,40,76,40),.close],"mane")
        case 1:
            for (x,y) in [(89.0,41.0),(98,49),(104,58)] { ellipse(x,y,3.4,3.4,"mane") }
        default:
            add([.m(76,36),.q(94,38,104,53),.l(101,57),.q(91,42,74,41),.close],"mane")
        }
        if identity.accessory == 0 { add([.m(76,36),.l(90,42),.l(83,48),.close],"c2543f") }
    }
}

func ranchColor(_ hex: Int) -> Color {
    Color(.sRGB,red:Double((hex >> 16)&255)/255,green:Double((hex >> 8)&255)/255,blue:Double(hex&255)/255,opacity:1)
}
