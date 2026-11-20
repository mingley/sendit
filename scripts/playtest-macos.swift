// Drive the packaged game through the same foreground Quartz event stream as a keyboard and mouse.
import AppKit
import CoreGraphics
import Metal

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data((message + "\n").utf8))
    exit(1)
}

func wait(_ seconds: TimeInterval) {
    RunLoop.current.run(until: Date(timeIntervalSinceNow: seconds))
}

let args = Array(CommandLine.arguments.dropFirst())
if args == ["status"] {
    let capabilities: [String: Any] = [
        "metal": MTLCreateSystemDefaultDevice()?.name ?? "",
        "login_session": CGSessionCopyCurrentDictionary() != nil,
        "post_events": CGPreflightPostEventAccess(),
    ]
    let data = try JSONSerialization.data(withJSONObject: capabilities)
    FileHandle.standardOutput.write(data)
    exit(0)
}

guard args.count == 2, let pid = Int32(args[1]),
      let target = NSRunningApplication(processIdentifier: pid),
      let source = CGEventSource(stateID: .hidSystemState) else {
    fail("Expected an action and the running Sendit process ID")
}
guard CGPreflightPostEventAccess() else {
    fail("Quartz event-post permission was revoked during the playtest")
}

func gameBounds() -> CGRect? {
    let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
    let windows = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [[String: Any]] ?? []
    for window in windows {
        guard (window[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value == pid,
              (window[kCGWindowLayer as String] as? NSNumber)?.intValue == 0,
              let b = window[kCGWindowBounds as String] as? [String: NSNumber],
              let x = b["X"], let y = b["Y"], let w = b["Width"], let h = b["Height"],
              w.doubleValue >= 200, h.doubleValue >= 160 else { continue }
        return CGRect(x: x.doubleValue, y: y.doubleValue, width: w.doubleValue, height: h.doubleValue)
    }
    return nil
}

func inFront() -> Bool {
    NSWorkspace.shared.frontmostApplication?.processIdentifier == pid
}

if !inFront() {
    _ = target.activate(options: [.activateIgnoringOtherApps])
}
let deadline = Date(timeIntervalSinceNow: 8)
while (!inFront() || gameBounds() == nil) && Date() < deadline {
    wait(0.1)
}
guard inFront(), let bounds = gameBounds() else { fail("Could not focus the packaged game window") }

func key(_ code: CGKeyCode, hold: TimeInterval = 0.16) {
    guard inFront(),
          let down = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: true),
          let up = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: false)
    else { fail("Could not send a key to the foreground game") }
    down.post(tap: .cghidEventTap)
    wait(hold)
    up.post(tap: .cghidEventTap)
    wait(0.12)
}

// This point stays inside the game, clear of both centered menus and the window title bar.
let point = CGPoint(x: bounds.minX + bounds.width * 0.12, y: bounds.minY + bounds.height * 0.34)
func mouse(_ kind: CGEventType) {
    guard inFront(), let event = CGEvent(
        mouseEventSource: source, mouseType: kind, mouseCursorPosition: point, mouseButton: .left
    ) else { fail("Could not send a mouse event to the foreground game") }
    event.post(tap: .cghidEventTap)
}

func click(hold: TimeInterval) {
    mouse(.mouseMoved)
    wait(0.10)
    mouse(.leftMouseDown)
    wait(hold)
    mouse(.leftMouseUp)
}

switch args[0] {
case "start":
    click(hold: 0.16)
case "play":
    key(22)                 // ANSI number-row 6 selects the SMG.
    key(13, hold: 0.36)    // W moves forward through the captured window.
    click(hold: 0.52)
case "quit":
    mouse(.mouseMoved)
    key(53)                // Escape opens the in-game menu with Resume selected.
    mouse(.mouseMoved)
    wait(0.22)
    key(125)               // Down selects Quit to desktop.
    key(36)                // Return confirms.
default:
    fail("Unknown playtest action \(args[0])")
}
