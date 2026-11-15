import SwiftUI

/// One shared set of design tokens. Neutral grays only: hierarchy comes from weight and contrast.
enum Space {
    static let xxs: CGFloat = 2
    static let xs: CGFloat = 4
    static let s: CGFloat = 8
    static let m: CGFloat = 12
    static let l: CGFloat = 16
    static let xl: CGFloat = 24
    static let xxl: CGFloat = 32
    static let xxxl: CGFloat = 48
}

/// Shape rule: actions are capsules or circles; content cards and bubbles `card`; thumbnails and chips `m`;
/// the composer `composer` (a capsule while it is one line); sheets and the board `panel`.
enum Radius {
    static let s: CGFloat = 8
    static let m: CGFloat = 12
    static let card: CGFloat = 20
    static let l: CGFloat = 24
    static let composer: CGFloat = 28
    static let panel: CGFloat = 32
}

enum Stroke {
    static let hairline: CGFloat = 1
    static let thin: CGFloat = 1.5
    static let regular: CGFloat = 2
    static let bold: CGFloat = 3
}

enum Size {
    /// Row height of the floating controls in Live.
    static let control: CGFloat = 56
    /// Round buttons over the camera (top bar and both sides of the pill): one size so the row reads as a set.
    static let liveButton: CGFloat = 56
    static let pillWidth: CGFloat = 200
    static let pillHeight: CGFloat = 64
    static let icon: CGFloat = 17
    /// Minimum tap target and single-line row.
    static let tap: CGFloat = 44
    /// A list row with a title and a subtitle.
    static let row: CGFloat = 52
}

/// Type scale: system sans for everything the app says about itself (chrome, lists, data); the serif is
/// Beluga's own voice (the greeting, and assistant answers in `chat.html`).
enum TypeScale {
    static let greeting = Font.system(size: 30, weight: .regular, design: .serif)
    static let title = Font.system(size: 28, weight: .semibold)
    static let display = Font.system(size: 34, weight: .semibold).monospacedDigit()
    static let headline = Font.system(size: 17, weight: .semibold)
    static let body = Font.system(size: 17, weight: .regular)
    static let callout = Font.system(size: 15, weight: .regular)
    static let caption = Font.system(size: 13, weight: .regular)
    static let micro = Font.system(size: 11, weight: .medium)
}

/// Colours that are not ink. The pill's light is one token: swap `glow` to retint it.
enum Palette {
    /// The light inside the voice pill (monochrome by design).
    static let glow = Color(white: 0.96)
    /// The dark the light reads against, in both appearances.
    static let scrim = Color.black
    /// Dim layer behind the drawer, at full open.
    static let dim = Color.black.opacity(0.28)
}

/// Neutral ink levels that adapt to light/dark.
enum Ink {
    static let primary = Color.primary
    static let secondary = Color.primary.opacity(0.68)
    static let tertiary = Color.primary.opacity(0.55)
    static let faint = Color.primary.opacity(0.14)
    static let hairline = Color.primary.opacity(0.10)
    static let wash = Color.primary.opacity(0.06)
}

/// Motion vocabulary. Every animation in the app is one of these springs (no bounce unless a finger threw it),
/// so state changes share one rhythm and stay interruptible: a spring retargets from its current velocity.
///
/// - `snappy`  0.26 s: small state changes in place (button morphs, chips, ticks, symbols, fades).
/// - `smooth`  0.42 s: things that travel (drawer, board, Live over chat, sheets of content, tab cross-fades).
/// - `gentle`  0.7 s:  the rare entrance (home greeting and doodle).
/// - `release(velocity:distance:)`: the end of a drag; carries the finger's speed into `smooth`, with a
///   little bounce only when the flick was fast.
enum Motion {
    static let snappy = Animation.spring(duration: 0.26, bounce: 0)
    static let smooth = Animation.spring(duration: 0.42, bounce: 0)
    static let gentle = Animation.spring(duration: 0.7, bounce: 0)
    /// Kept for older call sites: the same spring as `smooth`.
    static let panel = smooth

    /// A spring that starts at the finger's speed. `velocity` in points per second, `distance` the points still
    /// to travel (signed the same way).
    static func release(velocity: CGFloat, distance: CGFloat) -> Animation {
        guard abs(distance) > 0.5 else { return smooth }
        let v = max(-12, min(12, velocity / distance))
        let fast = abs(velocity) > 900
        return .interpolatingSpring(duration: 0.42, bounce: fast ? 0.12 : 0, initialVelocity: v)
    }

    /// With Reduce Motion, travel becomes a quick cross-fade-length spring and nothing bounces.
    static func respecting(_ reduce: Bool, _ animation: Animation) -> Animation {
        reduce ? snappy : animation
    }

    /// Rubber band past a boundary: the further you pull, the less it follows (never more than `limit`).
    static func rubberBand(_ x: CGFloat, limit: CGFloat = 80) -> CGFloat {
        let sign: CGFloat = x < 0 ? -1 : 1
        let d = abs(x)
        return sign * limit * (1 - 1 / (d / limit * 0.55 + 1))
    }

    // Plain durations still used by timing-based effects (web view, glow smoothing).
    static let quick: Double = 0.18
    static let standard: Double = 0.32
    static let slow: Double = 0.6
}

/// Press feedback for custom buttons: scales to 0.97 on touch-down with `snappy`, back on release.
struct PressScaleStyle: ButtonStyle {
    var scale: CGFloat = 0.97
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed ? scale : 1)
            .opacity(configuration.isPressed ? 0.85 : 1)
            .animation(Motion.snappy, value: configuration.isPressed)
    }
}
