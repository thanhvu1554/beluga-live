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

enum Radius {
    static let s: CGFloat = 8
    static let m: CGFloat = 12
    static let l: CGFloat = 24
    static let panel: CGFloat = 32
}

enum Stroke {
    static let hairline: CGFloat = 1
    static let thin: CGFloat = 1.5
    static let regular: CGFloat = 2
    static let bold: CGFloat = 3
}

enum Size {
    /// Row height of the floating controls; round buttons are `control - Space.s` (48 pt, above the 44 pt minimum).
    static let control: CGFloat = 56
    static let pillWidth: CGFloat = 200
    static let pillHeight: CGFloat = 64
    static let icon: CGFloat = 17
}

/// Type scale: system font only, a few weights.
enum TypeScale {
    static let title = Font.system(size: 28, weight: .semibold)
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
}

/// Neutral ink levels that adapt to light/dark.
enum Ink {
    static let primary = Color.primary
    static let secondary = Color.primary.opacity(0.68)
    static let tertiary = Color.primary.opacity(0.55)
    static let faint = Color.primary.opacity(0.14)
    static let hairline = Color.primary.opacity(0.10)
}

/// Motion durations, kept short and calm.
enum Motion {
    static let quick: Double = 0.18
    static let standard: Double = 0.32
    static let slow: Double = 0.6
    static let panel = Animation.spring(response: 0.42, dampingFraction: 0.9)
}
