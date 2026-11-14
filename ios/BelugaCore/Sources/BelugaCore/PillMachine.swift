import Foundation

/// Visible states of the pill (PROTOCOL section 5).
public enum PillState: String, Sendable, CaseIterable {
    case idle, listening, thinking, tool, speaking, asking, done, offline
}

public enum PillInput: Sendable, Equatable {
    case connected
    case disconnected
    case micOpened
    /// Mic closed; `sent` is true when an utterance was sent to the server.
    case micClosed(sent: Bool)
    /// A typed or recognized utterance was sent without the mic (e.g. mock).
    case userSent
    /// Server `state` frame.
    case server(state: String, tool: String?)
    case sayStarted
    case sayFinished
    case askReached
    case doneReached
    /// The short "done" beat is over.
    case doneSettled
    case turnEnded
    case interrupted
    /// The server heard nothing usable (`stt_empty`): back to waiting, asking again if we were.
    case utteranceRejected
}

/// Derives the pill state from a few independent facts, so ordering of inputs rarely matters.
public struct PillMachine: Sendable {
    public private(set) var online = false
    public private(set) var micOpen = false
    public private(set) var speaking = false
    public private(set) var tool: String?
    public private(set) var awaitingReply = false
    public private(set) var asking = false
    public private(set) var doneBeat = false

    public private(set) var state: PillState = .offline
    public private(set) var enteredAt: Date
    private var thinkingAnnounced = false
    private var askingBeforeSend = false

    public static let thinkingAnnounceDelay: TimeInterval = 4

    public init(now: Date = Date()) { enteredAt = now }

    /// Returns true when the visible state changed.
    @discardableResult
    public mutating func handle(_ input: PillInput, now: Date = Date()) -> Bool {
        switch input {
        case .connected: online = true
        case .disconnected:
            online = false; speaking = false; tool = nil; awaitingReply = false
        case .micOpened:
            micOpen = true; askingBeforeSend = asking; doneBeat = false
        case .micClosed(let sent):
            micOpen = false
            if sent { awaitingReply = true; askingBeforeSend = asking || askingBeforeSend; asking = false }
        case .userSent:
            awaitingReply = true; asking = false; doneBeat = false
        case let .server(s, t):
            switch s {
            case "thinking": awaitingReply = true; tool = nil
            case "tool": tool = t ?? "tool"
            case "idle", "done": awaitingReply = false; tool = nil
            default: break
            }
        case .sayStarted:
            speaking = true; awaitingReply = false; tool = nil; asking = false; askingBeforeSend = false
        case .sayFinished: speaking = false
        case .askReached: asking = true; awaitingReply = false
        case .doneReached: doneBeat = true; asking = false
        case .doneSettled: doneBeat = false
        case .turnEnded: awaitingReply = false; tool = nil
        case .interrupted: speaking = false; awaitingReply = false; tool = nil
        case .utteranceRejected:
            awaitingReply = false; asking = askingBeforeSend; askingBeforeSend = false
        }
        return update(now: now)
    }

    private mutating func update(now: Date) -> Bool {
        let next: PillState
        if !online { next = .offline }
        else if micOpen { next = .listening }
        else if speaking { next = .speaking }
        else if tool != nil { next = .tool }
        else if awaitingReply { next = .thinking }
        else if asking { next = .asking }
        else if doneBeat { next = .done }
        else { next = .idle }
        guard next != state else { return false }
        state = next
        enteredAt = now
        thinkingAnnounced = false
        return true
    }

    /// True exactly once per thinking period, after 4 seconds of thinking.
    public mutating func shouldAnnounceThinking(now: Date = Date()) -> Bool {
        guard state == .thinking, !thinkingAnnounced,
              now.timeIntervalSince(enteredAt) >= Self.thinkingAnnounceDelay else { return false }
        thinkingAnnounced = true
        return true
    }
}
