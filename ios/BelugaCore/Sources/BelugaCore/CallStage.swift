import Foundation

/// One board block turned into plain, native-renderable text (the call screen's stage and the PiP window cannot
/// host the web board, so formulas become readable Unicode and tables a compact grid).
public struct StageBlock: Sendable, Equatable, Identifiable {
    public enum Kind: String, Sendable, Equatable {
        /// Display math, chemistry, a plotted function: one or more lines of formula text.
        case formula
        /// Prose (markdown flattened).
        case text
        /// The final result of the step: shown with emphasis.
        case answer
        /// `rows[0]` is the header when `hasHeader`.
        case table
        /// Code lines, monospaced.
        case code
    }

    public var id: String
    public var kind: Kind
    public var lines: [String]
    public var rows: [[String]]
    public var hasHeader: Bool
    public var note: String?

    public init(id: String, kind: Kind, lines: [String] = [], rows: [[String]] = [], hasHeader: Bool = false, note: String? = nil) {
        self.id = id; self.kind = kind; self.lines = lines; self.rows = rows; self.hasHeader = hasHeader; self.note = note
    }

    /// Converts a `show` block. Known types get a structured rendering; anything else uses its `fallback`
    /// markdown; nil when there is nothing readable (an SVG or an interactive block without fallback).
    public static func from(_ block: JSONValue, seq: Int = 0) -> StageBlock? {
        let id = block["id"]?.nonEmptyString ?? "s\(seq)"
        let note = block["note"]?.nonEmptyString.map(PlainMath.inline)
        let fallback = block["fallback"]?.nonEmptyString
        func fromFallback(_ kind: Kind = .text) -> StageBlock? {
            guard let fallback else { return nil }
            let lines = PlainMath.paragraphs(fallback)
            return lines.isEmpty ? nil : StageBlock(id: id, kind: kind, lines: lines, note: note)
        }
        switch block["type"]?.string ?? "" {
        case "math", "chem":
            guard var tex = block["tex"]?.nonEmptyString else { return fromFallback(.formula) }
            if block["type"]?.string == "chem" { tex = tex.replacingOccurrences(of: "<->", with: "⇌").replacingOccurrences(of: "->", with: "→") }
            let lines = PlainMath.text(fromTeX: tex).split(separator: "\n").map(String.init).filter { !$0.isEmpty }
            return lines.isEmpty ? fromFallback(.formula) : StageBlock(id: id, kind: .formula, lines: lines, note: note)
        case "text":
            guard let md = block["md"]?.nonEmptyString else { return fromFallback() }
            return StageBlock(id: id, kind: .text, lines: PlainMath.paragraphs(md), note: note)
        case "answer":
            guard let md = block["md"]?.nonEmptyString else { return fromFallback(.answer) }
            return StageBlock(id: id, kind: .answer, lines: PlainMath.paragraphs(md), note: note)
        case "table":
            let head = (block["head"]?.array ?? []).map { PlainMath.inline($0.string ?? "") }
            let rows = (block["rows"]?.array ?? []).map { ($0.array ?? []).map { PlainMath.inline($0.string ?? "") } }
            guard !head.isEmpty || !rows.isEmpty else { return fromFallback() }
            return StageBlock(id: id, kind: .table, rows: head.isEmpty ? rows : [head] + rows, hasHeader: !head.isEmpty, note: note)
        case "code":
            let code = block["code"]?.string ?? ""
            let lines = code.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
            return code.isEmpty ? fromFallback() : StageBlock(id: id, kind: .code, lines: Array(lines.prefix(12)), note: note)
        case "plot":
            let fns = (block["fn"]?.array ?? []).compactMap(\.nonEmptyString)
            guard !fns.isEmpty else { return fromFallback(.formula) }
            let lines = fns.map { "y = " + PlainMath.text(fromTeX: $0.replacingOccurrences(of: "*", with: "")) }
            return StageBlock(id: id, kind: .formula, lines: lines, note: note)
        case "vocab":
            guard let word = block["word"]?.nonEmptyString else { return fromFallback() }
            var head = word
            if let ipa = block["ipa"]?.nonEmptyString { head += "  /" + ipa.trimmingCharacters(in: CharacterSet(charactersIn: "/")) + "/" }
            if let pos = block["pos"]?.nonEmptyString { head += "  (" + pos + ")" }
            var lines = [head]
            if let m = block["meaning"]?.nonEmptyString { lines.append(PlainMath.inline(m)) }
            if let ex = block["example"]?.nonEmptyString { lines.append("“" + PlainMath.inline(ex) + "”") }
            return StageBlock(id: id, kind: .text, lines: lines, note: note)
        case "timeline":
            let items = (block["items"]?.array ?? []).map { item in
                [item["when"]?.string ?? "", PlainMath.inline(item["what"]?.string ?? "")].filter { !$0.isEmpty }.joined(separator: " — ")
            }.filter { !$0.isEmpty }
            return items.isEmpty ? fromFallback() : StageBlock(id: id, kind: .text, lines: items, note: note)
        default:
            return fromFallback()
        }
    }
}

/// What the call shows of the lesson: the current step and its blocks, the line being spoken, the open question.
/// Fed with the same board events as the web board (`step`, `show` with `step >= 0`, `ask`, `done`).
public struct CallStage: Sendable, Equatable {
    public private(set) var stepIndex: Int = -1
    public private(set) var stepTitle: String = ""
    public private(set) var blocks: [StageBlock] = []
    /// The sentence being spoken (or the last one), or what the student said.
    public var caption: String = ""
    /// Question of an `ask` waiting for the student's answer.
    public var prompt: String?
    public private(set) var finished = false

    public init() {}

    public var isEmpty: Bool { stepIndex < 0 && blocks.isEmpty }

    public mutating func reset() { self = CallStage() }

    public mutating func apply(_ e: LessonEvent) {
        switch e.kind {
        case let .step(index, title):
            guard index >= stepIndex else { return }
            if index != stepIndex { blocks = [] }
            stepIndex = index
            stepTitle = PlainMath.inline(title)
            finished = false
        case let .show(step, block):
            guard step >= 0, step >= stepIndex else { return }
            if step > stepIndex { stepIndex = step; stepTitle = ""; blocks = [] }
            guard let b = StageBlock.from(block, seq: e.seq) else { return }
            if let i = blocks.firstIndex(where: { $0.id == b.id }) { blocks[i] = b } else { blocks.append(b) }
        case let .ask(_, p):
            prompt = p.isEmpty ? nil : PlainMath.inline(p)
        case .done:
            finished = true
            prompt = nil
        default:
            break
        }
    }

    /// The block to put forward: the step's answer when it has one, else the latest block.
    public var featured: StageBlock? {
        blocks.last(where: { $0.kind == .answer }) ?? blocks.last
    }

    /// Up to `limit` other blocks of the step, oldest first (context above the featured one).
    public func supporting(limit: Int = 2) -> [StageBlock] {
        guard let f = featured else { return [] }
        return Array(blocks.filter { $0.id != f.id }.suffix(limit))
    }

    /// The line under the content: the open question while it waits, else the spoken caption.
    public var line: String {
        if let prompt, !prompt.isEmpty { return prompt }
        return caption
    }
}

/// Who is talking, for the call's indicator (derived from the pill state).
public enum CallPresence: String, Sendable, Equatable, CaseIterable {
    case listening, speaking, thinking, asking, waiting, muted, offline

    public static func from(_ pill: PillState, muted: Bool) -> CallPresence {
        switch pill {
        case .offline: return .offline
        case .speaking: return .speaking
        case .thinking, .tool: return .thinking
        case .listening: return muted ? .muted : .listening
        case .asking: return muted ? .muted : .asking
        case .idle, .done: return muted ? .muted : .waiting
        }
    }
}

/// TeX and inline markdown flattened to readable Unicode text (x², x₁, (5−1)/2, √x, Δ, ∞, ≤ ...).
public enum PlainMath {
    /// Display TeX to text. `\\` line breaks become "\n".
    public static func text(fromTeX tex: String) -> String {
        var p = TeXParser(Array(tex))
        let raw = p.parse(untilBrace: false)
        return raw.split(separator: "\n", omittingEmptySubsequences: false)
            .map { tidy(String($0)) }
            .filter { !$0.isEmpty }
            .joined(separator: "\n")
    }

    /// Inline markdown with `$…$` / `$$…$$` math: one line of plain text.
    public static func inline(_ md: String) -> String {
        var out = ""
        var rest = Substring(md)
        while let start = rest.firstIndex(of: "$") {
            out += stripMarkdown(String(rest[..<start]))
            var after = rest[rest.index(after: start)...]
            let display = after.first == "$"
            if display { after = after.dropFirst() }
            let close = display ? after.range(of: "$$") : after.range(of: "$")
            guard let close else { out += stripMarkdown(String(rest[start...])); rest = ""; break }
            out += text(fromTeX: String(after[..<close.lowerBound])).replacingOccurrences(of: "\n", with: " ")
            rest = after[close.upperBound...]
        }
        out += stripMarkdown(String(rest))
        return tidy(out)
    }

    /// Markdown paragraphs (blank-line separated; list items each their own line), each flattened.
    public static func paragraphs(_ md: String) -> [String] {
        var out: [String] = []
        var current: [String] = []
        func flush() {
            let joined = current.joined(separator: " ")
            if !joined.trimmingCharacters(in: .whitespaces).isEmpty { out.append(inline(joined)) }
            current = []
        }
        for raw in md.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = raw.trimmingCharacters(in: .whitespaces)
            if line.isEmpty { flush(); continue }
            if line.hasPrefix("- ") || line.hasPrefix("* ") || line.hasPrefix("+ ") {
                flush(); current = ["• " + line.dropFirst(2)]; flush(); continue
            }
            current.append(line)
        }
        flush()
        return out.filter { !$0.isEmpty }
    }

    static func stripMarkdown(_ s: String) -> String {
        var t = s.replacingOccurrences(of: "**", with: "").replacingOccurrences(of: "__", with: "")
            .replacingOccurrences(of: "`", with: "")
        t = t.replacingOccurrences(of: "*", with: "")
        while t.hasPrefix("#") { t.removeFirst() }
        return t
    }

    static func tidy(_ s: String) -> String {
        var out = ""
        var lastSpace = false
        for ch in s {
            if ch == " " || ch == "\t" {
                if !lastSpace { out.append(" ") }
                lastSpace = true
            } else {
                out.append(ch)
                lastSpace = false
            }
        }
        return out.trimmingCharacters(in: .whitespaces)
    }

    static let symbols: [String: String] = [
        "alpha": "α", "beta": "β", "gamma": "γ", "delta": "δ", "epsilon": "ε", "varepsilon": "ε", "theta": "θ",
        "lambda": "λ", "mu": "μ", "pi": "π", "rho": "ρ", "sigma": "σ", "tau": "τ", "phi": "φ", "varphi": "φ",
        "omega": "ω", "Delta": "Δ", "Gamma": "Γ", "Omega": "Ω", "Sigma": "Σ", "Pi": "Π", "Phi": "Φ", "Lambda": "Λ",
        "infty": "∞", "cdot": "·", "times": "×", "div": "÷", "pm": "±", "mp": "∓",
        "le": "≤", "leq": "≤", "ge": "≥", "geq": "≥", "ne": "≠", "neq": "≠", "approx": "≈", "equiv": "≡", "sim": "∼",
        "cup": "∪", "cap": "∩", "in": "∈", "notin": "∉", "subset": "⊂", "subseteq": "⊆", "emptyset": "∅", "varnothing": "∅",
        "Rightarrow": "⇒", "Leftarrow": "⇐", "Leftrightarrow": "⇔", "iff": "⇔", "implies": "⇒", "rightarrow": "→", "to": "→",
        "leftarrow": "←", "leftrightarrow": "↔", "mapsto": "↦", "forall": "∀", "exists": "∃", "sum": "Σ", "prod": "Π",
        "int": "∫", "partial": "∂", "nabla": "∇", "circ": "°", "degree": "°", "angle": "∠", "perp": "⊥", "parallel": "∥",
        "ldots": "…", "dots": "…", "cdots": "⋯", "quad": "\u{2003}", "qquad": "\u{2003}\u{2003}",
        "lt": "<", "gt": ">", "neg": "¬", "land": "∧", "lor": "∨", "triangle": "△", "star": "⋆", "%": "%",
        "ell": "ℓ", "hbar": "ℏ", "prime": "′",
    ]
    static let invisible: Set<String> = [
        "left", "right", "big", "Big", "bigg", "Bigg", "bigl", "bigr", "Bigl", "Bigr", "displaystyle", "textstyle",
        "limits", "nolimits", "middle", "nonumber", "notag",
    ]
    static let passThrough: Set<String> = [
        "text", "mathrm", "textbf", "mathbf", "mathit", "textit", "operatorname", "boldsymbol", "mathsf", "mathtt",
        "overline", "bar", "underline", "widetilde", "tilde", "mbox", "textrm", "pmb",
    ]
    static let superscripts: [Character: Character] = [
        "0": "⁰", "1": "¹", "2": "²", "3": "³", "4": "⁴", "5": "⁵", "6": "⁶", "7": "⁷", "8": "⁸", "9": "⁹",
        "+": "⁺", "-": "⁻", "−": "⁻", "=": "⁼", "(": "⁽", ")": "⁾", "n": "ⁿ", "i": "ⁱ", "x": "ˣ", "y": "ʸ",
        "a": "ᵃ", "b": "ᵇ", "k": "ᵏ", "m": "ᵐ", "t": "ᵗ", "°": "°", "′": "′", "*": "*",
    ]
    static let subscripts: [Character: Character] = [
        "0": "₀", "1": "₁", "2": "₂", "3": "₃", "4": "₄", "5": "₅", "6": "₆", "7": "₇", "8": "₈", "9": "₉",
        "+": "₊", "-": "₋", "−": "₋", "=": "₌", "(": "₍", ")": "₎", "a": "ₐ", "e": "ₑ", "o": "ₒ", "x": "ₓ",
        "i": "ᵢ", "j": "ⱼ", "n": "ₙ", "k": "ₖ", "m": "ₘ", "t": "ₜ", "h": "ₕ", "l": "ₗ", "p": "ₚ", "s": "ₛ",
    ]
}

/// A small recursive reader for the TeX the tutor writes (fractions, roots, scripts, symbols, environments).
struct TeXParser {
    let c: [Character]
    var i = 0

    init(_ c: [Character]) { self.c = c }

    mutating func parse(untilBrace: Bool) -> String {
        var out = ""
        while i < c.count {
            let ch = c[i]
            switch ch {
            case "}":
                i += 1
                if untilBrace { return out }
            case "{":
                i += 1
                out += parse(untilBrace: true)
            case "\\":
                out += command()
            case "^", "_":
                i += 1
                out += script(argument(), sup: ch == "^")
            case "&", "~":
                i += 1
                out += " "
            case "-":
                i += 1
                out += "−"
            default:
                i += 1
                out.append(ch)
            }
        }
        return out
    }

    /// One argument: a braced group, a command, or a single character.
    mutating func argument() -> String {
        while i < c.count, c[i] == " " { i += 1 }
        guard i < c.count else { return "" }
        if c[i] == "{" { i += 1; return parse(untilBrace: true) }
        if c[i] == "\\" { return command() }
        let ch = c[i]
        i += 1
        return ch == "-" ? "−" : String(ch)
    }

    mutating func command() -> String {
        i += 1 // the backslash
        guard i < c.count else { return "" }
        let first = c[i]
        guard first.isLetter else {
            i += 1
            switch first {
            case "\\": return "\n"
            case ",", ";", ":", " ", ">": return " "
            case "!": return ""
            default: return String(first) // \{ \} \% \$ \#
            }
        }
        var name = ""
        while i < c.count, c[i].isLetter { name.append(c[i]); i += 1 }
        switch name {
        case "frac", "dfrac", "tfrac", "cfrac":
            let a = argument(), b = argument()
            return wrap(a) + "/" + wrap(b)
        case "sqrt":
            var index = ""
            if i < c.count, c[i] == "[" {
                i += 1
                while i < c.count, c[i] != "]" { index.append(c[i]); i += 1 }
                i += 1
            }
            let a = argument()
            let root = index == "3" ? "∛" : index == "4" ? "∜" : (index.isEmpty ? "" : script(index, sup: true)) + "√"
            return root + wrap(a)
        case "vec": return argument() + "\u{20D7}"
        case "hat": return argument() + "\u{0302}"
        case "begin", "end":
            _ = argument()
            return name == "begin" ? "" : " "
        case "mathbb":
            let a = argument()
            return ["R": "ℝ", "N": "ℕ", "Z": "ℤ", "Q": "ℚ", "C": "ℂ"][a] ?? a
        case "not":
            let a = argument()
            return a == "=" ? "≠" : a == "∈" ? "∉" : a + "\u{0338}"
        default:
            if PlainMath.passThrough.contains(name) { return argument() }
            if PlainMath.invisible.contains(name) {
                if i < c.count, c[i] == "." { i += 1 } // \left. or \right.
                return ""
            }
            return PlainMath.symbols[name] ?? name
        }
    }

    func script(_ s: String, sup: Bool) -> String {
        let map = sup ? PlainMath.superscripts : PlainMath.subscripts
        let t = s.trimmingCharacters(in: .whitespaces)
        guard !t.isEmpty else { return "" }
        let mapped = t.compactMap { map[$0] }
        if mapped.count == t.count { return String(mapped) }
        let grouped = t.count == 1 || (t.hasPrefix("(") && t.hasSuffix(")")) ? t : "(" + t + ")"
        return (sup ? "^" : "_") + grouped
    }

    func wrap(_ s: String) -> String {
        let t = s.trimmingCharacters(in: .whitespaces)
        if t.count <= 1 || t.allSatisfy({ $0.isLetter || $0.isNumber || $0 == "." || $0 == "," }) { return t }
        if t.hasPrefix("("), t.hasSuffix(")") { return t }
        return "(" + t + ")"
    }
}
