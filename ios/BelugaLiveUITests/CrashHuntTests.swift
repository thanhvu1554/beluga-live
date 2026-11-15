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
        launch(["-mockOffer", "1", "-mockAudioProbe", "1"])
        tap("Cài đặt")
        pause(2)
        alive("settings open")
        tapContaining("Giọng gia sư")
        pause(2)
        let probe = app.descendants(matching: .any).matching(identifier: "audio.probe").firstMatch
        if probe.waitForExistence(timeout: 3) { print("HUNT AUDIOPROBE \(probe.label)") } else { print("HUNT AUDIOPROBE missing") }
        // The mock plays the server's real ID3-tagged clip through the same download-check-cache-play path.
        tapContaining("Nghe thử giọng")
        pause(1.2)
        alive("voice preview")
        XCTAssertTrue(exists("Dừng nghe thử giọng"), "the ID3-tagged clip plays (the button turned into Stop)")
        XCTAssertFalse(exists("Không tải được đoạn nghe thử", timeout: 0.5), "no failure under the voice")
        pause(6)
        XCTAssertFalse(exists("Dừng nghe thử giọng", timeout: 0.5), "the clip ended by itself and the button is back")
        alive("voice preview ended")
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

    /// PROTOCOL section 17: a folder with a free name and a line about it, created from the drawer, opened at
    /// once, then its about edited from the folder's menu.
    func testCreateAndEditFolder() {
        launch([])
        tap("Thư mục và phiên")
        pause(1.5)
        tap("Thư mục mới")
        pause(1.2)
        alive("new folder sheet")
        let name = field("folder.name")
        XCTAssertTrue(app.keyboards.count > 0, "keyboard should rise with the sheet")
        name.typeText("Tiếng Nhật N5")
        field("folder.about").tap()
        pause(0.4)
        app.typeText("Luyện kanji và nghe")
        pause(0.6)
        alive("new folder filled")
        tap("Tạo")
        pause(2.5)
        alive("new folder opened")
        XCTAssertTrue(exists("Tiếng Nhật N5"), "the new folder opens by its own name")
        XCTAssertTrue(exists("Luyện kanji và nghe"), "the about line shows in the folder")
        tap("Thêm")
        pause(1)
        tap("Sửa thư mục")
        pause(1.2)
        let about = field("folder.about")
        about.tap()
        pause(0.3)
        app.typeText(", mỗi tối 20 phút")
        pause(0.5)
        tap("Lưu")
        pause(1.5)
        alive("folder edited")
        XCTAssertTrue(exists("Luyện kanji và nghe, mỗi tối 20 phút"), "edited about shows at once")
        app.navigationBars.buttons.firstMatch.tap()
        pause(1.5)
        tap("Thư mục và phiên")
        pause(1.5)
        XCTAssertTrue(exists("Tiếng Nhật N5"), "the drawer lists the folder by its name")
        alive("drawer with new folder")
    }

    /// PROTOCOL section 18: the root home names each folder on its cards; inside a folder the cards only ever
    /// speak of that folder (the old home suggested Math while the owner was in English).
    func testHomeSuggestionsStayInTheirFolder() {
        launch([])
        pause(1.5)
        XCTAssertTrue(exists("Ôn 2 mục tới hạn"), "root home offers the due review")
        XCTAssertTrue(exists("Toán 9 · Giữa kỳ 1"), "root cards carry the folder's name")
        alive("root home cards")
        tap("Thư mục và phiên")
        pause(1.5)
        tapContaining("Tiếng Anh trên lớp")
        pause(2)
        tap("Đoạn chat mới")
        pause(2.5)
        alive("home inside English")
        XCTAssertTrue(exists("Ôn 1 mục tới hạn"), "the folder's own due review")
        XCTAssertTrue(exists("Tiếng Anh trên lớp"), "the folder's own name")
        XCTAssertFalse(exists("Toán 9", timeout: 1.5), "nothing of the Math folder inside English")
    }

    /// PROTOCOL section 19: an online launch fills the replica; without the server the app shows that copy
    /// (folders, a folder's sessions, a session's transcript), says so quietly, and queues a new folder, which
    /// reaches the server on the next launch with the server back.
    func testOfflineReplicaAndQueuedEdits() {
        launch(["-mockFreshReplica", "1"])
        pause(4)
        alive("online, replica filled")
        app.terminate()
        launch(["-mockUnreachable", "1"])
        pause(2)
        alive("offline launch")
        XCTAssertTrue(exists("Ngoại tuyến"), "the home title says offline")
        tap("Thư mục và phiên")
        pause(1.5)
        XCTAssertTrue(exists("Đang xem bản lưu lúc"), "the drawer says the screens show the saved copy")
        XCTAssertTrue(exists("Toán 9 · Giữa kỳ 1"), "folders come from the replica")
        alive("offline drawer")
        tapContaining("Toán 9 · Giữa kỳ 1")
        pause(2)
        XCTAssertTrue(exists("Rút gọn biểu thức chứa căn"), "the folder's sessions come from the replica")
        tap("Ôn tập")
        pause(1.5)
        alive("offline review tab")
        tap("Phiên")
        pause(1)
        tapContaining("Hệ thức Vi-ét")
        pause(2.5)
        alive("offline session from the replica")
        tap("Thư mục và phiên")
        pause(1.5)
        tap("Thư mục mới")
        pause(1.2)
        field("folder.name").typeText("Lịch sử 9")
        pause(0.4)
        tap("Tạo")
        pause(2)
        alive("folder created offline")
        XCTAssertTrue(exists("Lịch sử 9"), "the offline folder opens at once")
        app.navigationBars.buttons.firstMatch.tap()
        pause(1.5)
        tap("Thư mục và phiên")
        pause(1.5)
        XCTAssertTrue(exists("Chờ đồng bộ"), "the offline folder is marked as waiting")
        XCTAssertTrue(exists("1 thay đổi chờ gửi"), "the drawer counts the waiting edit")
        alive("offline drawer with a queued folder")
        app.terminate()
        launch([])
        pause(4)
        tap("Thư mục và phiên")
        pause(1.5)
        XCTAssertTrue(exists("Lịch sử 9"), "the queued folder reached the server")
        XCTAssertFalse(exists("Chờ đồng bộ", timeout: 1), "nothing waits any more")
        alive("online again, edit replayed")
    }

    /// PROTOCOL section 22: the call rings ("Đang gọi Beluga…"), connects ("Đã kết nối", then the timer),
    /// Beluga picks up with a hello line in its own voice, and the red button shows "Đã kết thúc" before closing.
    func testCallRingsConnectsAndEnds() {
        launch([])
        tapContaining("Gọi gia sư")
        pause(0.5)
        XCTAssertTrue(exists("Đang gọi Beluga", timeout: 1.5), "the call starts by ringing")
        alive("call dialing")
        XCTAssertTrue(exists("Đã kết nối", timeout: 4), "then it connects")
        alive("call connected")
        XCTAssertTrue(anyExists(["A lô, tớ nghe đây", "Tớ đây, mình học gì nào?"], timeout: 3), "Beluga picks up with a hello line")
        pause(2.5)
        XCTAssertFalse(exists("Đang gọi Beluga", timeout: 0.5), "no ringing once connected")
        alive("call running")
        tap("Kết thúc")
        XCTAssertTrue(exists("Đã kết thúc", timeout: 1.5), "the ended beat")
        alive("call ended beat")
        pause(2)
        XCTAssertTrue(exists("Hỏi Beluga", timeout: 3), "back in the chat")
        alive("back from the call")
    }

    /// Without the server the dial gives up after 8 s with "Không gọi được" and "Gọi lại"; hanging up still works.
    func testCallFailsWithoutServer() {
        launch(["-mockUnreachable", "1"])
        tapContaining("Gọi gia sư")
        pause(1)
        XCTAssertTrue(exists("Đang gọi Beluga", timeout: 1.5), "it rings first")
        XCTAssertTrue(exists("Không gọi được", timeout: 10), "then says it could not connect")
        XCTAssertTrue(app.buttons["Gọi lại"].exists, "and offers to call again")
        alive("call failed")
        tap("Gọi lại")
        pause(1)
        XCTAssertTrue(exists("Đang gọi Beluga", timeout: 1.5), "calling again rings again")
        tap("Kết thúc")
        pause(2.5)
        alive("hung up while dialing")
    }

    /// PROTOCOL section 21: in a call, when the tutor thinks for more than 3 s a short line in its voice fills the
    /// wait, and it is gone the moment the tutor's real answer starts.
    func testWaitClipWhileThinking() {
        launch(["-mockCall", "1", "-mockSlowThink", "1"])
        let shorts = ["Ừm, để tớ xem nào", "Chờ tớ một chút nhé", "Để tớ nghĩ xíu"]
        pause(11)
        XCTAssertTrue(anyExists(shorts, timeout: 6), "a wait line plays while the tutor thinks")
        alive("wait clip")
        pause(7)
        XCTAssertFalse(anyExists(shorts, timeout: 0.5), "the wait line is gone once the tutor speaks")
        alive("tutor speaking after the wait")
    }

    /// Settings > Dữ liệu: the phone's copy (synced, when) and the server's R2 backup (last, next, "Sao lưu ngay").
    func testDataSettings() {
        launch([])
        pause(2)
        tap("Cài đặt")
        pause(1.5)
        tapContaining("Dữ liệu")
        pause(1.5)
        alive("data settings")
        XCTAssertTrue(exists("Đã đồng bộ"), "the copy on the phone is in sync")
        XCTAssertTrue(exists("Lần sao lưu cuối"), "the last backup is shown")
        tap("Sao lưu ngay")
        pause(0.6)
        XCTAssertTrue(exists("Đang sao lưu"), "the button shows the backup running")
        alive("backup running")
        pause(3)
        XCTAssertTrue(app.buttons["Sao lưu ngay"].waitForExistence(timeout: 3), "back to idle once the server reports it")
        alive("backup done")
        app.navigationBars.buttons.firstMatch.tap()
        pause(1)
        tap("Xong")
        pause(1.5)
        alive("settings closed")
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

    /// A text field or text view by its accessibility identifier, focused.
    @discardableResult
    private func field(_ id: String) -> XCUIElement {
        let el = app.descendants(matching: .any).matching(identifier: id).firstMatch
        guard el.waitForExistence(timeout: 4) else {
            print("HUNT MISSING field: \(id)")
            XCTFail("field not found: \(id)")
            return el
        }
        if !el.hasKeyboardFocusSafe { el.tap() }
        pause(0.3)
        return el
    }

    private func anyExists(_ texts: [String], timeout: TimeInterval) -> Bool {
        let format = texts.map { _ in "label CONTAINS[c] %@" }.joined(separator: " OR ")
        let predicate = NSPredicate(format: format, argumentArray: texts)
        return app.descendants(matching: .any).matching(predicate).firstMatch.waitForExistence(timeout: timeout)
    }

    private func exists(_ text: String, timeout: TimeInterval = 4) -> Bool {
        let predicate = NSPredicate(format: "label CONTAINS[c] %@ OR value CONTAINS[c] %@", text, text)
        return app.descendants(matching: .any).matching(predicate).firstMatch.waitForExistence(timeout: timeout)
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

private extension XCUIElement {
    var hasKeyboardFocusSafe: Bool { (value(forKey: "hasKeyboardFocus") as? Bool) ?? false }
}
