// Drive the packaged game through the same foreground Quartz event stream as a keyboard and mouse.
import AppKit
import CoreGraphics
import Metal

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data((message + "\n").utf8))
    exit(1)
}

func wait(_ seconds: TimeInterval) {
    let deadline = Date(timeIntervalSinceNow: seconds)
    RunLoop.current.run(until: deadline)
    // A command-line run loop can return early when it has no live sources.
    if deadline.timeIntervalSinceNow > 0 { Thread.sleep(until: deadline) }
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

func keyEvents(_ code: CGKeyCode) -> (CGEvent, CGEvent) {
    guard inFront(),
          let down = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: true),
          let up = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: false)
    else { fail("Could not send a key to the foreground game") }
    return (down, up)
}

func key(_ code: CGKeyCode, hold: TimeInterval = 0.16) {
    let (down, up) = keyEvents(code)
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

// Keep the OS key or button down in this process until Python sees it in the game's recording.
func awaitRelease() {
    FileHandle.standardOutput.write(Data("held\n".utf8))
    _ = readLine()
}

switch args[0] {
case "start":
    click(hold: 0.16)
case "equip":
    key(22, hold: 0.22)    // ANSI number-row 6 selects the SMG.
case "walk", "strafe":
    let (down, up) = keyEvents(args[0] == "walk" ? 13 : 2) // ANSI W and D.
    down.post(tap: .cghidEventTap)
    awaitRelease()
    up.post(tap: .cghidEventTap)
case "fire":
    mouse(.leftMouseDown)
    awaitRelease()
    mouse(.leftMouseUp)
case "pause":
    mouse(.mouseMoved)
    key(53, hold: 0.26)    // Escape opens the in-game menu with Resume selected.
case "quit":
    key(125, hold: 0.26)   // Down selects Quit to desktop.
    wait(0.50)
    key(36, hold: 0.26)    // Return confirms.
default:
    fail("Unknown playtest action \(args[0])")
}
