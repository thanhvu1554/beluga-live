import SwiftUI
import BelugaCore

/// Small playful line drawings next to the greeting (PROTOCOL section 15). Stroke only, in the current
/// foreground style. On arrival the lines draw themselves in once (0.9 s), then loop gently at 30 fps while
/// the drawing is on screen (`active`); off screen, under Live or behind the drawer the clock stops, so the
/// Canvas costs nothing. Reduce Motion shows one still frame.
struct DoodleView: View {
    let doodle: Doodle
    var active = true
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var start = Date.distantFuture
    @State private var started = false

    private static let drawIn: Double = 0.9

    var body: some View {
        TimelineView(.animation(minimumInterval: 1.0 / 30, paused: reduceMotion || !active)) { context in
            let t = reduceMotion ? 0.6 : context.date.timeIntervalSinceReferenceDate
            let k = reduceMotion ? 1 : min(1, max(0, context.date.timeIntervalSince(start) / Self.drawIn))
            // Ease-out: fast at first, settling as the last stroke closes.
            let reveal = 1 - pow(1 - k, 3)
            Canvas { ctx, size in
                let style = StrokeStyle(lineWidth: 1.6, lineCap: .round, lineJoin: .round)
                // Drawings are authored on a 160 x 96 box.
                let k = min(size.width / 160, size.height / 96)
                ctx.translateBy(x: (size.width - 160 * k) / 2, y: (size.height - 96 * k) / 2)
                ctx.scaleBy(x: k, y: k)
                for line in Self.paths(doodle, t: t) {
                    let path = reveal < 1 ? line.path.trimmedPath(from: 0, to: reveal) : line.path
                    ctx.stroke(path, with: .foreground, style: line.dashed ? StrokeStyle(lineWidth: 1.2, lineCap: .round, dash: [2, 5]) : style)
                }
            }
        }
        // The draw-in starts when the drawing is first on screen (not when the view value was made).
        .onAppear { if !started { started = true; start = Date() } }
        .onChange(of: doodle) { _, _ in start = Date() }
    }

    struct Line { var path: Path; var dashed = false }

    static func paths(_ d: Doodle, t: Double) -> [Line] {
        switch d {
        case .catWalk: return catWalk(t)
        case .catSleep: return catSleep(t)
        case .whaleSwim: return whale(t)
        case .birdHop: return bird(t)
        case .snail: return snail(t)
        case .paperPlane: return plane(t)
        }
    }

    // MARK: Drawings

    private static func ground(_ y: CGFloat = 84) -> Line {
        Line(path: Path { p in p.move(to: CGPoint(x: 14, y: y)); p.addLine(to: CGPoint(x: 146, y: y)) }, dashed: true)
    }

    /// A cat strolling back and forth along a line: legs swing, tail sways, head bobs.
    private static func catWalk(_ t: Double) -> [Line] {
        let cycle = t.truncatingRemainder(dividingBy: 12) / 12          // 0...1 over 12 s
        let dir: CGFloat = cycle < 0.5 ? 1 : -1
        let u = cycle < 0.5 ? cycle * 2 : (1 - cycle) * 2
        let x = 40 + CGFloat(u) * 70
        let step = sin(t * 7)
        let bob = CGFloat(abs(sin(t * 7))) * 1.2
        var p = Path()
        // Body.
        p.addEllipse(in: CGRect(x: x - 22, y: 50 - bob, width: 44, height: 22))
        // Head with ears, on the walking side.
        let hx = x + dir * 26, hy = 46 - bob
        p.addEllipse(in: CGRect(x: hx - 9, y: hy - 8, width: 18, height: 16))
        p.move(to: CGPoint(x: hx - 7, y: hy - 4)); p.addLine(to: CGPoint(x: hx - 6, y: hy - 13)); p.addLine(to: CGPoint(x: hx - 1, y: hy - 7))
        p.move(to: CGPoint(x: hx + 7, y: hy - 4)); p.addLine(to: CGPoint(x: hx + 6, y: hy - 13)); p.addLine(to: CGPoint(x: hx + 1, y: hy - 7))
        // Tail.
        let tx = x - dir * 22
        p.move(to: CGPoint(x: tx, y: 56 - bob))
        p.addQuadCurve(to: CGPoint(x: tx - dir * 10, y: 34 + CGFloat(sin(t * 2.4)) * 4),
                       control: CGPoint(x: tx - dir * 16, y: 54))
        // Legs.
        for (i, lx) in [x - 14, x - 6, x + 6, x + 14].enumerated() {
            let swing = CGFloat(step) * (i % 2 == 0 ? 4 : -4)
            p.move(to: CGPoint(x: lx, y: 68 - bob)); p.addLine(to: CGPoint(x: lx + swing, y: 84))
        }
        return [Line(path: p), ground()]
    }

    /// A curled-up cat breathing slowly, with z's drifting up.
    private static func catSleep(_ t: Double) -> [Line] {
        let breath = CGFloat(sin(t * 1.4)) * 1.5
        var p = Path()
        p.addEllipse(in: CGRect(x: 44, y: 52 - breath, width: 64, height: 32 + breath))
        p.addEllipse(in: CGRect(x: 90, y: 56 - breath, width: 22, height: 18))
        p.move(to: CGPoint(x: 94, y: 60 - breath)); p.addLine(to: CGPoint(x: 95, y: 51 - breath)); p.addLine(to: CGPoint(x: 100, y: 57 - breath))
        p.move(to: CGPoint(x: 108, y: 60 - breath)); p.addLine(to: CGPoint(x: 110, y: 51 - breath)); p.addLine(to: CGPoint(x: 104, y: 57 - breath))
        // Closed eye.
        p.move(to: CGPoint(x: 97, y: 66 - breath)); p.addQuadCurve(to: CGPoint(x: 103, y: 66 - breath), control: CGPoint(x: 100, y: 68 - breath))
        // Tail wrapped around.
        p.move(to: CGPoint(x: 48, y: 74)); p.addQuadCurve(to: CGPoint(x: 92, y: 82), control: CGPoint(x: 60, y: 90))
        var z = Path()
        for i in 0..<3 {
            let phase = (t * 0.35 + Double(i) / 3).truncatingRemainder(dividingBy: 1)
            let zx = 116 + CGFloat(phase) * 18, zy = 48 - CGFloat(phase) * 36, s = 4 + CGFloat(phase) * 3
            guard phase > 0.05, phase < 0.9 else { continue }
            z.move(to: CGPoint(x: zx, y: zy)); z.addLine(to: CGPoint(x: zx + s, y: zy))
            z.addLine(to: CGPoint(x: zx, y: zy + s)); z.addLine(to: CGPoint(x: zx + s, y: zy + s))
        }
        return [Line(path: p), Line(path: z), ground(86)]
    }

    /// A small whale bobbing on a wave, with a spout now and then.
    private static func whale(_ t: Double) -> [Line] {
        let bob = CGFloat(sin(t * 1.6)) * 3
        var w = Path()
        w.move(to: CGPoint(x: 46, y: 58 + bob))
        w.addCurve(to: CGPoint(x: 106, y: 62 + bob), control1: CGPoint(x: 50, y: 30 + bob), control2: CGPoint(x: 100, y: 32 + bob))
        w.addQuadCurve(to: CGPoint(x: 46, y: 58 + bob), control: CGPoint(x: 76, y: 80 + bob))
        // Tail flukes.
        let flick = CGFloat(sin(t * 3)) * 3
        w.move(to: CGPoint(x: 104, y: 60 + bob)); w.addLine(to: CGPoint(x: 120, y: 46 + bob - flick))
        w.addLine(to: CGPoint(x: 116, y: 58 + bob)); w.addLine(to: CGPoint(x: 126, y: 62 + bob + flick)); w.addLine(to: CGPoint(x: 106, y: 62 + bob))
        // Eye and smile.
        w.addEllipse(in: CGRect(x: 60, y: 50 + bob, width: 2.5, height: 2.5))
        w.move(to: CGPoint(x: 52, y: 60 + bob)); w.addQuadCurve(to: CGPoint(x: 62, y: 61 + bob), control: CGPoint(x: 57, y: 64 + bob))
        var spout = Path()
        let s = (t * 0.5).truncatingRemainder(dividingBy: 1)
        if s < 0.45 {
            let h = CGFloat(sin(s / 0.45 * .pi)) * 14
            spout.move(to: CGPoint(x: 72, y: 40 + bob)); spout.addLine(to: CGPoint(x: 72, y: 40 + bob - h))
            spout.move(to: CGPoint(x: 72, y: 40 + bob - h)); spout.addQuadCurve(to: CGPoint(x: 64, y: 38 + bob - h * 0.6), control: CGPoint(x: 66, y: 40 + bob - h))
            spout.move(to: CGPoint(x: 72, y: 40 + bob - h)); spout.addQuadCurve(to: CGPoint(x: 80, y: 38 + bob - h * 0.6), control: CGPoint(x: 78, y: 40 + bob - h))
        }
        var wave = Path()
        let shift = CGFloat((t * 12).truncatingRemainder(dividingBy: 24))
        wave.move(to: CGPoint(x: 10, y: 80))
        var x: CGFloat = 10
        while x < 150 {
            let y = 80 + sin((x + shift) / 24 * 2 * .pi) * 3
            wave.addLine(to: CGPoint(x: x, y: y)); x += 3
        }
        return [Line(path: w), Line(path: spout), Line(path: wave)]
    }

    /// A round bird hopping in place, turning its head.
    private static func bird(_ t: Double) -> [Line] {
        let phase = (t * 1.3).truncatingRemainder(dividingBy: 1)
        let hop = phase < 0.4 ? CGFloat(sin(phase / 0.4 * .pi)) * 12 : 0
        let x = 80 + CGFloat(sin(t * 0.4)) * 30
        let face: CGFloat = cos(t * 0.4) >= 0 ? 1 : -1
        var b = Path()
        b.addEllipse(in: CGRect(x: x - 14, y: 50 - hop, width: 28, height: 24))
        b.move(to: CGPoint(x: x + face * 13, y: 58 - hop)); b.addLine(to: CGPoint(x: x + face * 20, y: 61 - hop)); b.addLine(to: CGPoint(x: x + face * 13, y: 63 - hop))
        b.addEllipse(in: CGRect(x: x + face * 6 - 1.2, y: 56 - hop, width: 2.4, height: 2.4))
        // Wing and tail.
        b.move(to: CGPoint(x: x - face * 2, y: 62 - hop)); b.addQuadCurve(to: CGPoint(x: x - face * 10, y: 66 - hop), control: CGPoint(x: x - face * 8, y: 58 - hop))
        b.move(to: CGPoint(x: x - face * 13, y: 60 - hop)); b.addLine(to: CGPoint(x: x - face * 22, y: 54 - hop))
        // Legs.
        b.move(to: CGPoint(x: x - 4, y: 74 - hop)); b.addLine(to: CGPoint(x: x - 5, y: 84 - hop))
        b.move(to: CGPoint(x: x + 4, y: 74 - hop)); b.addLine(to: CGPoint(x: x + 5, y: 84 - hop))
        return [Line(path: b), ground()]
    }

    /// A snail inching along, eye stalks wobbling.
    private static func snail(_ t: Double) -> [Line] {
        let x = 30 + CGFloat((t * 4).truncatingRemainder(dividingBy: 90))
        let stretch = CGFloat(sin(t * 2)) * 2
        var s = Path()
        // Body.
        s.move(to: CGPoint(x: x - 18, y: 84))
        s.addLine(to: CGPoint(x: x + 22 + stretch, y: 84))
        s.addQuadCurve(to: CGPoint(x: x + 18 + stretch, y: 70), control: CGPoint(x: x + 28 + stretch, y: 76))
        // Stalks.
        let w = CGFloat(sin(t * 3)) * 2
        s.move(to: CGPoint(x: x + 20 + stretch, y: 71)); s.addLine(to: CGPoint(x: x + 22 + stretch + w, y: 58))
        s.move(to: CGPoint(x: x + 16 + stretch, y: 71)); s.addLine(to: CGPoint(x: x + 15 + stretch - w, y: 59))
        s.addEllipse(in: CGRect(x: x + 21 + stretch + w, y: 56, width: 3, height: 3))
        s.addEllipse(in: CGRect(x: x + 13.5 + stretch - w, y: 57, width: 3, height: 3))
        // Shell spiral.
        let c = CGPoint(x: x, y: 68)
        var a = 0.0, r: CGFloat = 16
        s.move(to: CGPoint(x: c.x + r, y: c.y))
        while r > 2 {
            a += 0.25; r -= 0.42
            s.addLine(to: CGPoint(x: c.x + cos(a) * r, y: c.y + sin(a) * r))
        }
        return [Line(path: s), ground(85)]
    }

    /// A paper plane looping gently with a dashed trail.
    private static func plane(_ t: Double) -> [Line] {
        func pos(_ tt: Double) -> CGPoint {
            CGPoint(x: 80 + CGFloat(sin(tt * 0.9)) * 52, y: 48 + CGFloat(sin(tt * 1.8)) * 18)
        }
        let p0 = pos(t), p1 = pos(t + 0.05)
        let angle = atan2(p1.y - p0.y, p1.x - p0.x)
        var plane = Path()
        let pts: [CGPoint] = [CGPoint(x: 12, y: 0), CGPoint(x: -10, y: -8), CGPoint(x: -5, y: 0), CGPoint(x: -10, y: 8), CGPoint(x: 12, y: 0), CGPoint(x: -5, y: 0)]
        let rot = CGAffineTransform(rotationAngle: angle)
        for (i, q) in pts.enumerated() {
            let r = q.applying(rot)
            let pt = CGPoint(x: p0.x + r.x, y: p0.y + r.y)
            if i == 0 { plane.move(to: pt) } else { plane.addLine(to: pt) }
        }
        var trail = Path()
        trail.move(to: pos(t - 1.4))
        var tt = t - 1.4
        while tt < t - 0.15 { tt += 0.05; trail.addLine(to: pos(tt)) }
        return [Line(path: plane), Line(path: trail, dashed: true)]
    }
}

/// The small whale mark (same shapes as the app icon), drawn as a filled glyph.
struct BelugaMark: View {
    var body: some View {
        Canvas { ctx, size in
            let k = min(size.width / 30, size.height / 20)
            ctx.translateBy(x: (size.width - 30 * k) / 2, y: (size.height - 20 * k) / 2)
            ctx.scaleBy(x: k, y: k)
            var body = Path()
            body.move(to: CGPoint(x: 2, y: 12))
            body.addCurve(to: CGPoint(x: 22, y: 13), control1: CGPoint(x: 3, y: 2), control2: CGPoint(x: 18, y: 3))
            body.addQuadCurve(to: CGPoint(x: 2, y: 12), control: CGPoint(x: 11, y: 21))
            body.move(to: CGPoint(x: 21, y: 12)); body.addLine(to: CGPoint(x: 28, y: 5)); body.addLine(to: CGPoint(x: 26, y: 11))
            body.addLine(to: CGPoint(x: 29, y: 15)); body.addLine(to: CGPoint(x: 22, y: 14)); body.closeSubpath()
            ctx.fill(body, with: .foreground)
            ctx.blendMode = .destinationOut
            ctx.fill(Path(ellipseIn: CGRect(x: 6.5, y: 8, width: 2.2, height: 2.2)), with: .color(.black))
        }
    }
}
