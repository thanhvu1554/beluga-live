import SwiftUI
import BelugaCore

/// Type sizes of the native lesson rendering: the call stage (full screen) and the PiP window (a 300 x 400 canvas
/// scaled to the window, so everything shrinks together and stays in proportion).
struct StageMetrics {
    var stepLabel: CGFloat
    var title: CGFloat
    var formula: CGFloat
    var text: CGFloat
    var table: CGFloat
    var caption: CGFloat
    var lineLimit: Int

    static let stage = StageMetrics(stepLabel: 12, title: 26, formula: 30, text: 19, table: 17, caption: 19, lineLimit: 8)
    static let pip = StageMetrics(stepLabel: 12, title: 21, formula: 26, text: 18, table: 15, caption: 17, lineLimit: 4)
}

/// Light ink on the call's dark ground (the call looks the same in light and dark appearance).
enum CallInk {
    static let primary = Palette.glow
    static let secondary = Palette.glow.opacity(0.72)
    static let tertiary = Palette.glow.opacity(0.5)
    static let hairline = Palette.glow.opacity(0.12)
    static let wash = Palette.glow.opacity(0.06)
    static let ground = Color(white: 0.055)
}

/// A line of math in the serif, with sub- and superscripts drawn in the same face (smaller, baseline-shifted)
/// rather than as Unicode script characters, which the serif lacks and would take from a sans fallback.
enum MathText {
    static func make(_ line: String, size: CGFloat, weight: Font.Weight = .regular) -> Text {
        var out = AttributedString()
        for run in PlainMath.scriptRuns(line) {
            var a = AttributedString(run.text)
            switch run.position {
            case .base:
                a.swiftUI.font = Font.system(size: size, weight: weight, design: .serif)
            case .sup:
                a.swiftUI.font = Font.system(size: size * 0.66, weight: weight, design: .serif)
                a.swiftUI.baselineOffset = size * 0.38
            case .sub:
                a.swiftUI.font = Font.system(size: size * 0.66, weight: weight, design: .serif)
                a.swiftUI.baselineOffset = -size * 0.16
            }
            out += a
        }
        return Text(out)
    }
}

/// One stage block, natively: formulas in a serif, prose, a compact table, the answer with emphasis.
struct StageBlockView: View {
    let block: StageBlock
    let metrics: StageMetrics
    var dimmed = false
    @Environment(\.strings) private var s

    var body: some View {
        content
            .foregroundStyle(dimmed ? CallInk.tertiary : CallInk.primary)
            .frame(maxWidth: .infinity, alignment: .leading)
    }

    @ViewBuilder private var content: some View {
        switch block.kind {
        case .formula:
            VStack(alignment: .leading, spacing: 6) {
                ForEach(Array(block.lines.prefix(4).enumerated()), id: \.offset) { _, line in
                    // One equation per line, never wrapped mid-way: it shrinks to fit instead.
                    MathText.make(line, size: dimmed ? metrics.formula * 0.7 : metrics.formula)
                        .lineLimit(1)
                        .minimumScaleFactor(0.4)
                }
                note
            }
        case .answer:
            VStack(alignment: .leading, spacing: 8) {
                Text(verbatim: s("call.answer").uppercased())
                    .font(.system(size: metrics.stepLabel, weight: .semibold))
                    .tracking(1.2)
                    .foregroundStyle(CallInk.tertiary)
                ForEach(Array(block.lines.prefix(3).enumerated()), id: \.offset) { _, line in
                    MathText.make(line, size: metrics.formula * 0.9, weight: .semibold)
                        .lineLimit(3)
                        .minimumScaleFactor(0.45)
                }
            }
            .padding(.vertical, 12)
            .padding(.horizontal, 16)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: Radius.m, style: .continuous).fill(CallInk.wash))
            .overlay(RoundedRectangle(cornerRadius: Radius.m, style: .continuous).strokeBorder(Palette.glow.opacity(0.35), lineWidth: Stroke.hairline))
        case .text:
            VStack(alignment: .leading, spacing: 6) {
                ForEach(Array(block.lines.prefix(4).enumerated()), id: \.offset) { _, line in
                    Text(verbatim: line)
                        .font(.system(size: dimmed ? metrics.text * 0.85 : metrics.text))
                        .lineLimit(dimmed ? 2 : metrics.lineLimit)
                        .minimumScaleFactor(0.7)
                }
                note
            }
        case .code:
            VStack(alignment: .leading, spacing: 2) {
                ForEach(Array(block.lines.prefix(8).enumerated()), id: \.offset) { _, line in
                    Text(verbatim: line.isEmpty ? " " : line)
                        .font(.system(size: metrics.table, design: .monospaced))
                        .lineLimit(1)
                        .minimumScaleFactor(0.6)
                }
            }
        case .table:
            StageTable(rows: block.rows, hasHeader: block.hasHeader, size: dimmed ? metrics.table * 0.85 : metrics.table)
        }
    }

    @ViewBuilder private var note: some View {
        if let note = block.note, !dimmed {
            Text(verbatim: note)
                .font(.system(size: metrics.stepLabel))
                .foregroundStyle(CallInk.tertiary)
                .lineLimit(1)
        }
    }
}

/// A small table: header row in semibold over a hairline, columns sized to their cells.
private struct StageTable: View {
    let rows: [[String]]
    let hasHeader: Bool
    let size: CGFloat

    var body: some View {
        let shown = Array(rows.prefix(6))
        Grid(alignment: .leading, horizontalSpacing: size * 0.9, verticalSpacing: size * 0.45) {
            ForEach(Array(shown.enumerated()), id: \.offset) { i, row in
                GridRow {
                    ForEach(Array(row.prefix(6).enumerated()), id: \.offset) { _, cell in
                        MathText.make(cell, size: size, weight: hasHeader && i == 0 ? .semibold : .regular)
                            .lineLimit(1)
                            .minimumScaleFactor(0.5)
                    }
                }
                if hasHeader && i == 0 && shown.count > 1 {
                    Rectangle().fill(CallInk.hairline).frame(height: Stroke.hairline).gridCellUnsizedAxes(.horizontal)
                }
            }
        }
    }
}

/// Symbol and words for who is talking.
struct PresenceLabel: View {
    let presence: CallPresence
    let size: CGFloat
    @Environment(\.strings) private var s
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        HStack(spacing: size * 0.45) {
            Image(systemName: Self.symbol(presence))
                .font(.system(size: size * 0.95, weight: .semibold))
                .symbolEffect(.variableColor.iterative, options: .repeating,
                              isActive: !reduceMotion && (presence == .speaking || presence == .thinking))
                .symbolEffect(.pulse, options: .repeating, isActive: !reduceMotion && presence == .listening)
                .contentTransition(.symbolEffect(.replace))
                .frame(width: size * 1.4)
            Text(verbatim: s("call.state.\(presence.rawValue)"))
                .font(.system(size: size, weight: .semibold))
                .contentTransition(.opacity)
                .lineLimit(1)
        }
        .foregroundStyle(presence == .offline || presence == .muted ? CallInk.secondary : CallInk.primary)
        .animation(Motion.snappy, value: presence)
    }

    static func symbol(_ p: CallPresence) -> String {
        switch p {
        case .speaking: return "waveform"
        case .listening: return "mic.fill"
        case .thinking: return "ellipsis"
        case .asking: return "mic"
        case .waiting: return "circle.dotted"
        case .muted: return "mic.slash.fill"
        case .offline: return "wifi.slash"
        }
    }
}

/// "12:34" since the call started, ticking once a second.
struct CallTimer: View {
    let start: Date?
    let size: CGFloat

    var body: some View {
        if let start {
            TimelineView(.periodic(from: start, by: 1)) { ctx in
                Text(verbatim: Self.format(ctx.date.timeIntervalSince(start)))
                    .font(.system(size: size, weight: .medium).monospacedDigit())
            }
        }
    }

    static func format(_ t: TimeInterval) -> String {
        let s = max(0, Int(t))
        return s >= 3600 ? String(format: "%d:%02d:%02d", s / 3600, (s / 60) % 60, s % 60)
                         : String(format: "%02d:%02d", s / 60, s % 60)
    }
}

/// What the PiP window shows, as plain values (the live window reads the controller; the mock gallery builds several).
struct CallSnapshot: Equatable {
    var stage: CallStage
    var presence: CallPresence
    var startedAt: Date?
    var cameraNote: String?
}

/// The PiP window: dark, legible at a third of the screen, nothing but the lesson and who is talking.
/// Laid out on a 300 x 400 canvas and scaled to the window, so a pinch-enlarged window just gets bigger.
struct PiPCard: View {
    let snapshot: CallSnapshot
    @Environment(\.strings) private var s

    var body: some View {
        GeometryReader { geo in
            let scale = min(geo.size.width / 300, geo.size.height / 400)
            ZStack {
                CallInk.ground
                canvas
                    .frame(width: 300, height: 400)
                    .scaleEffect(scale)
            }
            .frame(width: geo.size.width, height: geo.size.height)
        }
        .ignoresSafeArea()
        .environment(\.colorScheme, .dark)
    }

    private var stage: CallStage { snapshot.stage }

    private var canvas: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                PresenceLabel(presence: snapshot.presence, size: 15)
                Spacer(minLength: 4)
                if snapshot.cameraNote != nil {
                    Image(systemName: "video.slash.fill")
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(CallInk.tertiary)
                }
                CallTimer(start: snapshot.startedAt, size: 14)
                    .foregroundStyle(CallInk.tertiary)
            }
            .padding(.bottom, 14)

            if stage.isEmpty {
                Text(verbatim: "Beluga")
                    .font(.system(size: 30, weight: .regular, design: .serif))
                    .foregroundStyle(CallInk.primary)
                    .padding(.top, 24)
                Spacer(minLength: 0)
            } else {
                if stage.stepIndex >= 0 || !stage.stepTitle.isEmpty {
                    Text(verbatim: stepHeading)
                        .font(.system(size: StageMetrics.pip.title, weight: .semibold))
                        .foregroundStyle(CallInk.primary)
                        .lineLimit(2)
                        .minimumScaleFactor(0.8)
                        .id("t\(stage.stepIndex)")
                        .transition(.opacity)
                        .padding(.bottom, 14)
                }
                if let block = stage.featured {
                    StageBlockView(block: block, metrics: .pip)
                        .id(block.id)
                        .transition(.asymmetric(insertion: .opacity.combined(with: .offset(y: 8)), removal: .opacity))
                }
                Spacer(minLength: 8)
            }

            if !stage.line.isEmpty {
                Text(verbatim: stage.line)
                    .font(.system(size: StageMetrics.pip.caption))
                    .foregroundStyle(stage.prompt != nil ? CallInk.primary : CallInk.secondary)
                    .lineLimit(3)
                    .minimumScaleFactor(0.85)
                    .id(stage.line)
                    .transition(.opacity)
            }
        }
        .padding(18)
        .frame(width: 300, height: 400, alignment: .topLeading)
        .background(alignment: .bottom) {
            // The tutor's voice as a soft light rising from the bottom edge (static: the window may be tiny).
            LinearGradient(colors: [Palette.glow.opacity(0), Palette.glow.opacity(0.16)], startPoint: .top, endPoint: .bottom)
                .frame(height: 120)
                .opacity(snapshot.presence == .speaking ? 1 : snapshot.presence == .listening ? 0.5 : 0)
        }
        .animation(Motion.smooth, value: stage)
        .animation(Motion.smooth, value: snapshot.presence)
    }

    private var stepHeading: String {
        let n = stage.stepIndex >= 0 ? s("call.step", stage.stepIndex + 1) : ""
        if stage.stepTitle.isEmpty { return n }
        return n.isEmpty ? stage.stepTitle : n + " · " + stage.stepTitle
    }
}

/// The live PiP window content, following the controller.
struct CallPiPContent: View {
    let controller: LiveController

    var body: some View {
        PiPCard(snapshot: CallSnapshot(stage: controller.callStage, presence: controller.callPresence,
                                       startedAt: controller.callStartedAt,
                                       cameraNote: controller.cameraNoteKey == nil ? nil : "camera"))
            .environment(\.strings, controller.strings)
    }
}

/// Mock scene `pip`: the PiP window on its own at the sizes iOS gives it (small, medium, large), in three states,
/// over a stand-in home screen, so legibility can be judged from a screenshot.
struct PiPGallery: View {
    let controller: LiveController
    @Environment(\.colorScheme) private var scheme

    var body: some View {
        let shots = Self.snapshots()
        ZStack {
            LinearGradient(colors: scheme == .dark ? [Color(white: 0.12), Color(white: 0.2)] : [Color(white: 0.86), Color(white: 0.7)],
                           startPoint: .top, endPoint: .bottom)
                .ignoresSafeArea()
            VStack(alignment: .leading, spacing: Space.l) {
                HStack(alignment: .top, spacing: Space.l) {
                    tile(shots[0], width: 132)
                    tile(shots[1], width: 186)
                }
                tile(shots[2], width: 270)
            }
            .padding(Space.l)
        }
    }

    private func tile(_ snap: CallSnapshot, width: CGFloat) -> some View {
        PiPCard(snapshot: snap)
            .frame(width: width, height: width * 4 / 3)
            .clipShape(.rect(cornerRadius: 14, style: .continuous))
            .shadow(color: .black.opacity(0.35), radius: 14, y: 6)
    }

    static func snapshots() -> [CallSnapshot] {
        let events = MockScript.load().allEvents
        func stage(upTo seq: Int) -> CallStage {
            var st = CallStage()
            for e in events where e.seq <= seq && e.isBoardEvent { st.apply(e) }
            return st
        }
        let start = Date().addingTimeInterval(-754)
        var speaking = stage(upTo: 8)
        speaking.caption = "Hệ số a dương, nên đồ thị là parabol quay lên."
        var listening = stage(upTo: 12)
        listening.caption = "Đến đây cậu thử nghĩ một chút."
        listening.prompt = "Vậy f(x) dương trên những khoảng nào?"
        var answer = stage(upTo: 19)
        answer.caption = "Nhớ mẹo trong trái ngoài cùng là làm được cả dạng này."
        return [
            CallSnapshot(stage: speaking, presence: .speaking, startedAt: start, cameraNote: nil),
            CallSnapshot(stage: listening, presence: .listening, startedAt: start, cameraNote: "camera"),
            CallSnapshot(stage: answer, presence: .speaking, startedAt: start, cameraNote: nil),
        ]
    }
}
