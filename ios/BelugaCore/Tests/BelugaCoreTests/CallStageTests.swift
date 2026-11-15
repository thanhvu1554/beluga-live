import Foundation
import Testing
@testable import BelugaCore

private func step(_ seq: Int, _ index: Int, _ title: String) -> LessonEvent {
    LessonEvent(seq: seq, turn: 1, kind: .step(index: index, title: title))
}
private func show(_ seq: Int, step: Int, _ block: JSONValue) -> LessonEvent {
    LessonEvent(seq: seq, turn: 1, kind: .show(step: step, block: block))
}

@Suite struct PlainMathTests {
    @Test func powersFractionsAndSymbols() {
        #expect(PlainMath.text(fromTeX: "x^2 - 5x + 6 > 0") == "x² − 5x + 6 > 0")
        #expect(PlainMath.text(fromTeX: "\\Delta = (-5)^2 - 4 \\cdot 1 \\cdot 6 = 1") == "Δ = (−5)² − 4 · 1 · 6 = 1")
        #expect(PlainMath.text(fromTeX: "x_1 = \\frac{5-1}{2} = 2").hasPrefix("x₁ = (5−1)/2 = 2"))
        #expect(PlainMath.text(fromTeX: "\\sqrt{x+1} \\le \\frac{a}{b}") == "√(x+1) ≤ a/b")
        #expect(PlainMath.text(fromTeX: "x^{n+1}") == "xⁿ⁺¹")
        #expect(PlainMath.text(fromTeX: "e^{i\\pi}") == "e^(iπ)")
        #expect(PlainMath.text(fromTeX: "\\left( a \\right)") == "( a )")
        #expect(PlainMath.text(fromTeX: "\\text{với } x \\in \\mathbb{R}") == "với x ∈ ℝ")
    }

    @Test func lineBreaksAndEnvironments() {
        let t = PlainMath.text(fromTeX: "\\begin{cases} x > 1 \\\\ x < 3 \\end{cases}")
        #expect(t.split(separator: "\n").map(String.init) == ["x > 1", "x < 3"])
    }

    @Test func inlineMarkdownWithMath() {
        #expect(PlainMath.inline("$S = (-\\infty;\\, 2) \\cup (3;\\, +\\infty)$") == "S = (−∞; 2) ∪ (3; +∞)")
        #expect(PlainMath.inline("Mẹo: **trong trái, ngoài cùng** khi $a > 0$.") == "Mẹo: trong trái, ngoài cùng khi a > 0.")
        #expect(PlainMath.inline("Giá $5 thì sao") == "Giá $5 thì sao")
        #expect(PlainMath.paragraphs("Dòng một\nnối tiếp\n\n- ý a\n- ý b") == ["Dòng một nối tiếp", "• ý a", "• ý b"])
    }
}

@Suite struct CallStageTests {
    @Test func blocksConvertToReadableText() {
        let table = StageBlock.from(["type": "table", "id": "t", "head": ["$x$", "$(2;\\,3)$"], "rows": [["$f(x)$", "$-$"]]])
        #expect(table?.kind == .table)
        #expect(table?.hasHeader == true)
        #expect(table?.rows == [["x", "(2; 3)"], ["f(x)", "−"]])
        let plot = StageBlock.from(["type": "plot", "fn": ["x^2-5*x+6"]], seq: 9)
        #expect(plot?.id == "s9")
        #expect(plot?.lines == ["y = x²−5x+6"])
        let chem = StageBlock.from(["type": "chem", "tex": "2H_2 + O_2 -> 2H_2O"])
        #expect(chem?.lines == ["2H₂ + O₂ −> 2H₂O"] || chem?.lines == ["2H₂ + O₂ → 2H₂O"])
        // Unknown types use their markdown fallback; nothing readable means no block.
        let svg = StageBlock.from(["type": "svg", "svg": "<svg/>", "fallback": "Hình tam giác $ABC$"])
        #expect(svg?.lines == ["Hình tam giác ABC"])
        #expect(StageBlock.from(["type": "interactive", "html": "<b>x</b>"]) == nil)
    }

    @Test func featuredPrefersTheAnswerThenTheLatestBlock() {
        var s = CallStage()
        s.apply(step(1, 0, "Đọc đề"))
        s.apply(show(2, step: 0, ["type": "math", "id": "a", "tex": "x^2 > 0"]))
        s.apply(show(3, step: 0, ["type": "text", "id": "b", "md": "Tìm $S$."]))
        #expect(s.featured?.id == "b")
        #expect(s.supporting().map(\.id) == ["a"])
        s.apply(step(4, 1, "Kết luận"))
        #expect(s.blocks.isEmpty)
        #expect(s.stepTitle == "Kết luận")
        s.apply(show(5, step: 1, ["type": "answer", "id": "c", "md": "$S = \\emptyset$"]))
        s.apply(show(6, step: 1, ["type": "text", "id": "d", "md": "Mẹo nhớ"]))
        #expect(s.featured?.id == "c")
        #expect(s.featured?.lines == ["S = ∅"])
        #expect(s.supporting().map(\.id) == ["d"])
    }

    @Test func staleStepsAreIgnoredAndShowsCanOpenAStep() {
        var s = CallStage()
        s.apply(step(1, 2, "Xét dấu"))
        s.apply(show(2, step: 1, ["type": "math", "id": "old", "tex": "x"]))
        #expect(s.blocks.isEmpty)
        s.apply(show(3, step: 3, ["type": "math", "id": "new", "tex": "y"]))
        #expect(s.stepIndex == 3)
        #expect(s.stepTitle.isEmpty)
        #expect(s.featured?.id == "new")
        // A chat-inline block (step -1) never reaches the stage.
        s.apply(show(4, step: -1, ["type": "math", "id": "chat", "tex": "z"]))
        #expect(s.blocks.map(\.id) == ["new"])
    }

    @Test func lineShowsTheOpenQuestionThenTheCaption() {
        var s = CallStage()
        s.caption = "Đến đây cậu thử nghĩ một chút."
        #expect(s.line == "Đến đây cậu thử nghĩ một chút.")
        s.apply(LessonEvent(seq: 1, turn: 1, kind: .ask(kind: "next", prompt: "Vậy $f(x) > 0$ khi nào?")))
        #expect(s.line == "Vậy f(x) > 0 khi nào?")
        s.prompt = nil
        #expect(s.line == "Đến đây cậu thử nghĩ một chút.")
        s.apply(LessonEvent(seq: 2, turn: 1, kind: .done))
        #expect(s.finished)
    }

    @Test func presenceFollowsThePill() {
        #expect(CallPresence.from(.speaking, muted: true) == .speaking)
        #expect(CallPresence.from(.listening, muted: false) == .listening)
        #expect(CallPresence.from(.tool, muted: false) == .thinking)
        #expect(CallPresence.from(.idle, muted: true) == .muted)
        #expect(CallPresence.from(.asking, muted: false) == .asking)
        #expect(CallPresence.from(.offline, muted: false) == .offline)
    }
}
