import XCTest

/// Walks the app the way its owner does on a real phone (software keyboard, the server's opening review offer,
/// a rate-limited answer, model sheet, drawer, folders, Live, a call) and fails when the app dies on the way.
/// Runs against the mock transport; the simulator's crash report names the UIKit frames a device report cannot.
@MainActor
final class CrashHuntTests: XCTestCase {
    private var app: XCUIApplication!

    override func setUp() async throws {
        continueAfterFailure = true
        app = XCUIApplication()
        addUIInterruptionMonitor(withDescription: "system alert") { alert in
            for label in ["Allow", "OK", "Cho phép", "Allow While Using App", "Continue", "Tiếp tục"] where alert.buttons[label].exists {
                alert.buttons[label].tap()
                return true
            }
            return false
        }
    }

    // MARK: Flows

    /// The crash of 19:51:20: a chat that opens with the review offer, "hi" typed on the keyboard and sent, the
    /// plan's rate limit as the answer.
    func testTypeAndSendRateLimited() {
        launch(["-mockOffer", "1", "-mockRateLimit", "1"])
        typeAndSend("hi")
        pause(4)
        alive("after first send")
        typeAndSend("xin chào")
        pause(4)
        alive("after second send")
    }

    func testTypeAndSendThenNewChat() {
        launch(["-mockOffer", "1"])
        typeAndSend("hi")
        pause(7)
        alive("after answer")
        tap("Đoạn chat mới")
        pause(2)
        alive("after new chat")
        typeAndSend("giải giúp tớ bài này")
        pause(3)
        tap("Đoạn chat mới")
        pause(2)
        alive("after new chat mid answer")
    }

    /// Home without the offer: the first message turns the home screen into a transcript (title, bar buttons).
    func testFirstMessageFromHome() {
        launch([])
        typeAndSend("hi")
        pause(6)
        alive("home to transcript")
        focusField()
        app.typeText("abc")
        pause(1)
        for _ in 0..<3 { app.typeText(XCUIKeyboardKey.delete.rawValue) }
        pause(1)
        alive("type and erase")
    }

    func testModelSheet() {
        launch(["-mockOffer", "1"])
        tap("Thêm tệp và tuỳ chọn")
        pause(1)
        tapContaining("Sonnet")
        pause(2)
        alive("model sheet open")
        tapContaining("DeepSeek")
        pause(1)
        tapContaining("Haiku")
        pause(1)
        tap("Xong")
        pause(2)
        alive("model sheet closed")
        typeAndSend("hi")
        pause(4)
        alive("send after model change")
    }

    func testModelSheetFromHomeTitle() {
        launch([])
        tapContaining("Beluga, ")
        pause(2)
        alive("model sheet from title")
        tapContaining("DeepSeek")
        pause(1)
        app.swipeDown(velocity: .fast)
        pause(2)
        alive("model sheet swiped away")
    }

    func testSettingsAndVoices() {
        launch(["-mockOffer", "1"])
        tap("Cài đặt")
        pause(2)
        alive("settings open")
        tapContaining("Giọng gia sư")
        pause(2)
        tapContaining("Nghe thử giọng")
        pause(2)
        alive("voice preview")
        app.navigationBars.buttons.firstMatch.tap()
        pause(1)
        tap("Xong")
        pause(2)
        alive("settings closed")
    }

    func testDrawerAndFolder() {
        launch(["-mockOffer", "1"])
        focusField()
        app.typeText("dang go")
        tap("Thư mục và phiên")
        pause(2)
        alive("drawer open with keyboard")
        tapContaining("Toán 9")
        pause(2)
        alive("folder open")
        for tab in ["Kiến thức", "Ôn tập", "Đề", "Phiên", "Kiến thức"] {
            tap(tab)
            pause(1.5)
            alive("folder tab \(tab)")
        }
        app.navigationBars.buttons.firstMatch.tap()
        pause(2)
        alive("back from folder")
    }

    func testLiveAndCall() {
        launch(["-mockOffer", "1"])
        tapContaining("Chuyển sang Live")
        pause(4)
        alive("live")
        tap("Về đoạn chat")
        pause(2)
        alive("back to chat")
        tapContaining("Gọi gia sư")
        pause(5)
        alive("call")
        tap("Kết thúc")
        pause(2)
        alive("call ended")
    }

    func testBackgroundAndReturn() {
        launch(["-mockOffer", "1"])
        focusField()
        app.typeText("hi")
        XCUIDevice.shared.press(.home)
        pause(2)
        app.activate()
        pause(3)
        alive("back from background with keyboard")
        tap("Gửi")
        pause(4)
        alive("send after background")
    }

    // MARK: Helpers

    private func launch(_ extra: [String]) {
        app.launchArguments = ["-mock", "1", "-AppleLanguages", "(vi)", "-AppleLocale", "vi_VN"] + extra
        app.launch()
        pause(3)
        alive("launch")
    }

    private func pause(_ seconds: TimeInterval) {
        RunLoop.current.run(until: Date().addingTimeInterval(seconds))
    }

    private func alive(_ step: String) {
        let ok = app.state == .runningForeground
        print("HUNT \(ok ? "alive" : "DEAD") at: \(step)")
        let shot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        shot.name = step
        shot.lifetime = .keepAlways
        add(shot)
        XCTAssertTrue(ok, "app is not running at: \(step)")
    }

    private func focusField() {
        let candidates = [app.textViews.firstMatch, app.textFields.firstMatch]
        for field in candidates where field.waitForExistence(timeout: 3) {
            field.tap()
            pause(1)
            print("HUNT keyboard shown: \(app.keyboards.count > 0)")
            return
        }
        let byLabel = app.descendants(matching: .any).matching(NSPredicate(format: "label CONTAINS[c] %@ OR placeholderValue CONTAINS[c] %@", "Hỏi Beluga", "Hỏi Beluga")).firstMatch
        if byLabel.waitForExistence(timeout: 3) {
            byLabel.tap()
            pause(1)
            print("HUNT keyboard shown (by label): \(app.keyboards.count > 0)")
            return
        }
        print("HUNT MISSING composer field")
        print(app.debugDescription.prefix(6000))
        XCTFail("composer field not found")
    }

    private func typeAndSend(_ text: String) {
        focusField()
        app.typeText(text)
        pause(0.6)
        tap("Gửi")
    }

    private func tap(_ label: String) {
        let button = app.buttons[label].firstMatch
        if button.waitForExistence(timeout: 4) {
            if button.isHittable { button.tap() } else { button.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap() }
            print("HUNT tap: \(label)")
        } else {
            print("HUNT MISSING: \(label)")
            XCTFail("button not found: \(label)")
        }
    }

    private func tapContaining(_ text: String) {
        let predicate = NSPredicate(format: "label CONTAINS[c] %@", text)
        let element = app.descendants(matching: .any).matching(predicate).firstMatch
        if element.waitForExistence(timeout: 4) {
            if element.isHittable { element.tap() } else { element.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap() }
            print("HUNT tap containing: \(text)")
        } else {
            print("HUNT MISSING containing: \(text)")
            XCTFail("element not found: \(text)")
        }
    }
}
