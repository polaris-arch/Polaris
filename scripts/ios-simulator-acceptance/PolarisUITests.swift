import XCTest
import UIKit

/// Exercises the installed production App. It never creates a VPN profile or
/// changes system routes; tunnel acceptance is a separate result.
final class PolarisUITests: XCTestCase {
    private let app = XCUIApplication(bundleIdentifier: "com.polaris.app")
    private let destinations = [
        ["Home", "首页", "首頁"],
        ["Nodes", "节点", "節點"],
        ["Routing", "分流", "分流"],
        ["Activity", "活动", "活動"],
        ["Settings", "设置", "設定"],
    ]

    override func setUpWithError() throws {
        continueAfterFailure = false
        XCUIDevice.shared.orientation = .portrait
        app.launch()
        XCTAssertTrue(navigation(0).waitForExistence(timeout: 30), app.debugDescription)
    }

    /// Run in an actual OS window; reveal controls before implementing paired-app tiling gestures.
    func testInspectIPadSystemWindowControls() throws {
        XCTAssertEqual(UIDevice.current.userInterfaceIdiom, .pad)
        XCTAssertEqual(app.state, .runningForeground)
        capture("ipad-system-window-controls")
        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        capture("ipad-system-window-controls-springboard", application: springboard)
        let controls = springboard.buttons["window-controls:com.polaris.app"]
        let appeared = controls.waitForExistence(timeout: 5)
        capture("ipad-system-window-controls-before-hold", application: springboard)
        XCTAssertTrue(appeared && controls.isHittable,
            "Prepare a real OS window exposing Polaris window controls before inspecting its menu")
        controls.press(forDuration: 1)
        capture("ipad-system-window-controls-menu", application: springboard)
        capture("ipad-system-window-controls-menu-app") // No unknown tiling action is clicked.
    }

    /// Discover actual Window/Move & Resize menus while editing; no unseen layout action is clicked.
    func testInspectIPadWindowMenu() throws {
        XCTAssertEqual(UIDevice.current.userInterfaceIdiom, .pad)
        let name = openEditingDraft()
        name.tap()
        name.typeText(" Window menu inspection")
        let draft = name.value as? String
        XCTAssertTrue(draft?.contains("Window menu inspection") == true)
        let process = processMarker()
        assertWindowedEditing(name, draft: draft, process: process, stage: "ipad-window-menu-editing")
        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        let polarisMenu = springboard.windows.containing(.button, identifier: "Polaris").firstMatch
        if !polarisMenu.exists {
            let appName = springboard.statusBars.staticTexts.matching(
                NSPredicate(format: "label == %@", "Polaris")).firstMatch
            let appeared = appName.waitForExistence(timeout: 5)
            capture("ipad-window-menu-before-status-bar", application: springboard)
            XCTAssertEqual(app.state, .runningForeground)
            XCTAssertTrue(appeared && appName.isHittable, "The actual Polaris status-bar name must reveal its menu")
            appName.tap()
        }
        let menuAppeared = polarisMenu.waitForExistence(timeout: 5)
        capture("ipad-window-menu-revealed", application: springboard)
        XCTAssertTrue(menuAppeared, "The menu must belong to the observed Polaris App-name button")
        let windowMenu = polarisMenu.buttons.matching(NSPredicate(format: "label == %@", "窗口")).firstMatch
        let windowAppeared = windowMenu.waitForExistence(timeout: 5)
        capture("ipad-window-menu-before-open", application: springboard)
        XCTAssertTrue(windowAppeared && windowMenu.isHittable, "The observed native Window menu button is required")
        windowMenu.tap()
        capture("ipad-window-menu-popup", application: springboard)
        capture("ipad-window-menu-popup-app")
        let moveResize = polarisMenu.buttons.matching(NSPredicate(format: "label == %@", "移动与调整大小")).firstMatch
        let moveResizeAppeared = moveResize.waitForExistence(timeout: 5)
        capture("ipad-window-menu-before-move-resize", application: springboard)
        XCTAssertTrue(moveResizeAppeared && moveResize.isEnabled && moveResize.isHittable,
            "The observed Move & Resize button must be enabled and reachable")
        moveResize.tap()
        capture("ipad-window-move-resize-submenu", application: springboard)
        capture("ipad-window-move-resize-submenu-app")
        XCTAssertEqual(processMarker(), process)
        XCTAssertEqual(name.value as? String, draft)
        // Preserve the actual menu for discovery; never choose an unobserved submenu, save or connect.
    }

    /// Discover the native Dock while editing; no App icon or unseen layout action is used.
    func testInspectIPadDockWhileEditing() throws {
        XCTAssertEqual(UIDevice.current.userInterfaceIdiom, .pad)
        let name = openEditingDraft()
        name.tap()
        name.typeText(" Dock editing inspection")
        let draft = name.value as? String
        XCTAssertTrue(draft?.contains("Dock editing inspection") == true)
        let process = processMarker()
        assertWindowedEditing(name, draft: draft, process: process, stage: "ipad-dock-initial-editing")

        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        let screenElement = springboard.windows["SBSwitcherWindow:Main"]
        let appeared = screenElement.waitForExistence(timeout: 5)
        capture("ipad-dock-before-gesture-springboard", application: springboard)
        XCTAssertTrue(appeared, "The actual SpringBoard screen bounds are required")
        let screen = screenElement.frame
        XCTAssertGreaterThan(screen.width, 0)
        XCTAssertGreaterThan(screen.height, 120)
        XCTAssertGreaterThan(screen.height, screen.width, "Prepare the actual iPad in portrait")
        XCTAssertEqual(app.windows.firstMatch.frame.width, screen.width, accuracy: 1,
            "Prepare the actual full Polaris window before Dock inspection")
        XCTAssertEqual(app.webViews.firstMatch.frame.width, screen.width, accuracy: 1)
        let start = screenElement.coordinate(withNormalizedOffset: CGVector(
            dx: 0.5, dy: (screen.height - 2) / screen.height))
        let finish = screenElement.coordinate(withNormalizedOffset: CGVector(
            dx: 0.5, dy: (screen.height - 120) / screen.height))
        let geometry = XCTAttachment(string: "screen=\(screen); window=\(app.windows.firstMatch.frame); content=\(app.webViews.firstMatch.frame); origin.screenPoint=\(screenElement.coordinate(withNormalizedOffset: .zero).screenPoint); start.screenPoint=\(start.screenPoint); finish.screenPoint=\(finish.screenPoint); localStart=(\(screen.width / 2),\(screen.height - 2)); localFinish=(\(screen.width / 2),\(screen.height - 120))")
        geometry.name = "ipad-dock-native-gesture-coordinates"
        geometry.lifetime = .keepAlways
        add(geometry)
        start.press(forDuration: 0.1, thenDragTo: finish)
        capture("ipad-dock-after-gesture-springboard", application: springboard)
        capture("ipad-dock-after-gesture-app")
        XCTAssertEqual(app.state, .runningForeground, "A Home or AppSwitcher transition is not Dock inspection")
        XCTAssertEqual(processMarker(), process)
        XCTAssertEqual(name.value as? String, draft)
        // Read these real Dock/App-icon snapshots before adding any two-App drag action.
    }

    /// Inspect the real assistant menu when iPad has kept its software keyboard hidden.
    func testInspectIPadKeyboardControls() throws {
        XCTAssertEqual(UIDevice.current.userInterfaceIdiom, .pad)
        let name = openEditingDraft()
        name.tap()
        name.typeText(" Keyboard recovery inspection")
        capture("ipad-keyboard-controls-after-synthetic-input")
        XCTAssertTrue((name.value as? String)?.contains("Keyboard recovery inspection") == true)
        let visible = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in
            let keyboard = self.app.keyboards.firstMatch
            return keyboard.exists && keyboard.frame.height > 0
                && !self.app.windows.firstMatch.frame.intersection(keyboard.frame).isEmpty
        }, object: nil)
        let result = XCTWaiter.wait(for: [visible], timeout: 10)
        capture("ipad-keyboard-controls-before")
        if result == .completed { return } // Actual keyboard is already visible; no menu needed.
        let keyboardButton = app.otherElements["inputAssistantView"].buttons.matching(
            NSPredicate(format: "label IN %@", ["Keyboard", "键盘", "鍵盤"])).firstMatch
        let appeared = keyboardButton.waitForExistence(timeout: 5)
        capture("ipad-keyboard-controls-assistant")
        XCTAssertTrue(appeared && keyboardButton.isHittable,
            "A hidden software keyboard requires the observed native assistant control")
        keyboardButton.tap()
        capture("ipad-keyboard-controls-menu") // Discover real Show Keyboard label before implementing recovery.
    }

    private func choosePolarisWindowLayout(_ imageIdentifier: String, stage: String) {
        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        XCTAssertEqual(app.state, .runningForeground)
        let controls = springboard.buttons["window-controls:com.polaris.app"]
        let appeared = controls.waitForExistence(timeout: 5)
        capture(stage + "-before-controls", application: springboard)
        if appeared {
            XCTAssertTrue(controls.isHittable, "Actual collapsed Polaris window controls must be reachable")
            controls.press(forDuration: 1)
        } else {
            // Expanded controls can be an Other container. Use the actually observed Window menu instead.
            let polarisMenu = springboard.windows.containing(.button, identifier: "Polaris").firstMatch
            if !polarisMenu.exists {
                let appName = springboard.statusBars.staticTexts.matching(
                    NSPredicate(format: "label == %@", "Polaris")).firstMatch
                let appNameAppeared = appName.waitForExistence(timeout: 5)
                capture(stage + "-before-status-bar-menu", application: springboard)
                XCTAssertTrue(appNameAppeared && appName.isHittable,
                    "Only the observed foreground Polaris status-bar name may reveal its menu")
                appName.tap()
            }
            let menuAppeared = polarisMenu.waitForExistence(timeout: 5)
            capture(stage + "-full-window-menu", application: springboard)
            XCTAssertTrue(menuAppeared, "The full-window fallback must belong to the actual Polaris menu")
            let windowMenu = polarisMenu.buttons.matching(NSPredicate(format: "label == %@", "窗口")).firstMatch
            let windowAppeared = windowMenu.waitForExistence(timeout: 5)
            capture(stage + "-before-window-menu", application: springboard)
            XCTAssertTrue(windowAppeared && windowMenu.isEnabled && windowMenu.isHittable,
                "The observed native Window menu button is required")
            windowMenu.tap()
            let moveResize = polarisMenu.buttons.matching(NSPredicate(format: "label == %@", "移动与调整大小")).firstMatch
            let moveResizeAppeared = moveResize.waitForExistence(timeout: 5)
            capture(stage + "-before-move-resize", application: springboard)
            XCTAssertTrue(moveResizeAppeared && moveResize.isEnabled && moveResize.isHittable,
                "The observed native Move & Resize submenu entry is required")
            moveResize.tap()
        }
        let layout = springboard.buttons.containing(.image, identifier: imageIdentifier).firstMatch
        let layoutAppeared = layout.waitForExistence(timeout: 5)
        capture(stage + "-layout-menu", application: springboard)
        XCTAssertTrue(layoutAppeared && layout.isEnabled && layout.isHittable, "The observed native window-layout action is missing")
        layout.tap()
    }

    /// Setup only: verify an already full window, or restore a retained narrow one using actual Fill.
    func testPrepareIPadFullWindow() throws {
        XCTAssertEqual(UIDevice.current.userInterfaceIdiom, .pad)
        let beforeWindow = app.windows.firstMatch.frame
        let beforeContent = app.webViews.firstMatch.frame
        let process = processMarker()
        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        let screen = springboard.windows["SBSwitcherWindow:Main"].frame
        capture("ipad-full-window-initial")
        XCTAssertGreaterThan(screen.width, 0)
        let alreadyFull = abs(beforeWindow.width - screen.width) <= 1
            && abs(beforeContent.width - screen.width) <= 1
        if !alreadyFull { choosePolarisWindowLayout("rectangle.inset.filled", stage: "ipad-full-window") }
        var previous: [CGRect]?
        let filled = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in
            guard self.app.state == .runningForeground else { previous = nil; return false }
            let frames = [self.app.windows.firstMatch.frame, self.app.webViews.firstMatch.frame]
            guard !frames[0].isEmpty && !frames[1].isEmpty else { previous = nil; return false }
            let valid = alreadyFull
                ? abs(frames[0].width - screen.width) <= 1 && abs(frames[1].width - screen.width) <= 1
                : frames[0].width - beforeWindow.width > 80 && frames[1].width - beforeContent.width > 80
            guard valid else { previous = nil; return false }
            let equal = previous == frames
            previous = frames
            return equal
        }, object: nil)
        let result = XCTWaiter.wait(for: [filled], timeout: 10)
        let stage = alreadyFull ? "ipad-full-window-already-full" : "ipad-full-window-after-fill"
        capture(stage)
        capture(stage + "-springboard", application: springboard)
        XCTAssertEqual(result, .completed, alreadyFull
            ? "Already-full preparation requires two adjacent valid foreground Window/WebView frames"
            : "Fill must actually widen and settle both native App and WebView")
        XCTAssertEqual(processMarker(), process)
        let finalScreen = springboard.windows["SBSwitcherWindow:Main"].frame
        XCTAssertGreaterThan(finalScreen.width, 0)
        XCTAssertEqual(app.windows.firstMatch.frame.width, finalScreen.width, accuracy: 1)
        XCTAssertEqual(app.webViews.firstMatch.frame.width, finalScreen.width, accuracy: 1)
        // This changes OS window preparation only, not paired-window or keyboard acceptance.
    }

    /// Two real OS App windows, one unsaved Polaris draft; never save or launch a VPN.
    func testIPadPairedWindowsKeepsEditingDraft() throws {
        XCTAssertEqual(UIDevice.current.userInterfaceIdiom, .pad)
        let name = openEditingDraft()
        name.tap()
        name.typeText(" Paired window draft")
        let draft = name.value as? String
        XCTAssertTrue(draft?.contains("Paired window draft") == true)
        let process = processMarker()
        assertWindowedEditing(name, draft: draft, process: process, stage: "ipad-paired-initial")
        let initialNameHeight = name.frame.height
        let settings = XCUIApplication(bundleIdentifier: "com.apple.Preferences")
        settings.launch() // A second actual OS window; do not relaunch the App owning this draft.
        capture("ipad-paired-second-app", application: settings)
        XCTAssertEqual(settings.state, .runningForeground)
        app.activate()
        capture("ipad-paired-return-to-draft")
        XCTAssertEqual(processMarker(), process)
        XCTAssertEqual(name.value as? String, draft)
        let beforeWindow = app.windows.firstMatch.frame
        let beforeContent = app.webViews.firstMatch.frame
        choosePolarisWindowLayout("inset.filled.lefthalf.righthalf.rectangle", stage: "ipad-paired")
        var previous: [CGRect]?
        let paired = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in
            guard self.app.state == .runningForeground, settings.windows.firstMatch.exists else {
                previous = nil
                return false
            }
            let frames = [self.app.windows.firstMatch.frame, self.app.webViews.firstMatch.frame,
                settings.windows.firstMatch.frame]
            let equal = previous == frames
            previous = frames
            return equal && !frames[0].isEmpty && !frames[1].isEmpty && !frames[2].isEmpty
                && frames[0].intersection(frames[2]).isEmpty
                && abs(frames[0].width - beforeWindow.width) > 80
                && abs(frames[1].width - beforeContent.width) > 80
        }, object: nil)
        let pairedResult = XCTWaiter.wait(for: [paired], timeout: 10)
        capture("ipad-paired-after-layout")
        capture("ipad-paired-after-layout-settings", application: settings)
        capture("ipad-paired-after-layout-springboard",
            application: XCUIApplication(bundleIdentifier: "com.apple.springboard"))
        XCTAssertEqual(pairedResult, .completed, "Both real App windows must settle side by side after the preset")
        XCTAssertEqual(processMarker(), process)
        XCTAssertEqual(name.value as? String, draft)
        name.tap()
        assertWindowedEditing(name, draft: draft, process: process, stage: "ipad-paired-keyboard")
        let window = app.windows.firstMatch.frame
        let settingsWindow = settings.windows.firstMatch.frame
        let screen = XCUIApplication(bundleIdentifier: "com.apple.springboard").windows["SBSwitcherWindow:Main"].frame
        let geometry = XCTAttachment(string: "process=\(processMarker()); beforeWindow=\(beforeWindow); beforeContent=\(beforeContent); window=\(window); content=\(app.webViews.firstMatch.frame); settingsWindow=\(settingsWindow); screen=\(screen); initialNameHeight=\(initialNameHeight); name=\(name.frame)")
        geometry.name = "ipad-paired-final-native-geometry"
        geometry.lifetime = .keepAlways
        add(geometry)
        capture("ipad-paired-final-settings", application: settings)
        capture("ipad-paired-final-springboard",
            application: XCUIApplication(bundleIdentifier: "com.apple.springboard"))
        XCTAssertGreaterThan(screen.width, 0)
        XCTAssertTrue(screen.contains(window) && screen.contains(settingsWindow))
        XCTAssertFalse(window.isEmpty || settingsWindow.isEmpty)
        XCTAssertTrue(window.intersection(settingsWindow).isEmpty, "Overlapping floating windows are not side-by-side tiling")
        XCTAssertLessThan(window.width, screen.width * 0.8)
        XCTAssertLessThan(settingsWindow.width, screen.width * 0.8)
        XCTAssertEqual(window.minY, settingsWindow.minY, accuracy: 1)
        XCTAssertEqual(window.height, settingsWindow.height, accuracy: 1)
        XCTAssertGreaterThanOrEqual(window.height, screen.height * 0.65)
        XCTAssertGreaterThan(abs(window.width - beforeWindow.width), 80)
        XCTAssertGreaterThan(abs(app.webViews.firstMatch.frame.width - beforeContent.width), 80)
        // Reject uniform AppSwitcher scaling; actual font/reflow still requires the retained screenshots.
        XCTAssertGreaterThanOrEqual(name.frame.height, initialNameHeight - 1)
        XCTAssertTrue(settings.staticTexts.allElementsBoundByIndex.contains {
            !$0.frame.isEmpty && settingsWindow.contains($0.frame) && $0.isHittable
        }, "The second App needs real reachable content, not a stale AX window or preview")
        app.terminate() // Discard only this synthetic draft; leave OS-mode restoration to the operator.
    }

    /// Drag the observed Settings Dock icon into a second real window, then explicitly resume editing.
    func testIPadDockPairedWindowsKeepsEditingDraft() throws {
        XCTAssertEqual(UIDevice.current.userInterfaceIdiom, .pad)
        let name = openEditingDraft()
        name.tap()
        name.typeText(" Dock paired draft")
        let draft = (name.value as? String) ?? ""
        XCTAssertTrue(draft.contains("Dock paired draft"))
        let process = processMarker()
        assertWindowedEditing(name, draft: draft, process: process, stage: "ipad-dock-paired-initial")
        let initialNameHeight = name.frame.height
        let beforeWindow = app.windows.firstMatch.frame
        let beforeContent = app.webViews.firstMatch.frame
        let settings = XCUIApplication(bundleIdentifier: "com.apple.Preferences")
        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        let screenElement = springboard.windows["SBSwitcherWindow:Main"]
        let screenAppeared = screenElement.waitForExistence(timeout: 5)
        capture("ipad-dock-paired-before-reveal-springboard", application: springboard)
        XCTAssertTrue(screenAppeared, "The actual SpringBoard screen is required")
        let screen = screenElement.frame
        XCTAssertGreaterThan(screen.width, 20)
        XCTAssertGreaterThan(screen.height, 120)
        XCTAssertGreaterThan(screen.height, screen.width, "Prepare the actual full portrait Polaris window")
        XCTAssertEqual(beforeWindow.width, screen.width, accuracy: 1)
        XCTAssertEqual(beforeContent.width, screen.width, accuracy: 1)
        let revealStart = screenElement.coordinate(withNormalizedOffset: CGVector(
            dx: 0.5, dy: (screen.height - 2) / screen.height))
        let revealFinish = screenElement.coordinate(withNormalizedOffset: CGVector(
            dx: 0.5, dy: (screen.height - 120) / screen.height))
        let revealGeometry = XCTAttachment(string: "screen=\(screen); origin.screenPoint=\(screenElement.coordinate(withNormalizedOffset: .zero).screenPoint); start.screenPoint=\(revealStart.screenPoint); finish.screenPoint=\(revealFinish.screenPoint)")
        revealGeometry.name = "ipad-dock-paired-reveal-coordinates"
        revealGeometry.lifetime = .keepAlways
        add(revealGeometry)
        revealStart.press(forDuration: 0.1, thenDragTo: revealFinish)
        let settingsIcon = springboard.otherElements["Multitasking Dock"].icons["设置"]
        let iconAppeared = settingsIcon.waitForExistence(timeout: 5)
        capture("ipad-dock-paired-before-icon-drag-springboard", application: springboard)
        capture("ipad-dock-paired-before-icon-drag-app")
        XCTAssertEqual(app.state, .runningForeground)
        XCTAssertEqual(processMarker(), process)
        XCTAssertEqual(name.value as? String, draft)
        XCTAssertTrue(iconAppeared && settingsIcon.isEnabled && settingsIcon.isHittable,
            "The observed Settings icon must be reachable in the actual Multitasking Dock")
        let dragStart = settingsIcon.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
        let dragFinish = screenElement.coordinate(withNormalizedOffset: CGVector(
            dx: (screen.width - 20) / screen.width, dy: 0.5))
        let dragGeometry = XCTAttachment(string: "screen=\(screen); beforeWindow=\(beforeWindow); beforeContent=\(beforeContent); icon=\(settingsIcon.frame); start.screenPoint=\(dragStart.screenPoint); finish.screenPoint=\(dragFinish.screenPoint); localFinish=(\(screen.width - 20),\(screen.height / 2)); press=1s; velocity=slow; hold=1s")
        dragGeometry.name = "ipad-dock-paired-icon-drag-coordinates"
        dragGeometry.lifetime = .keepAlways
        add(dragGeometry)
        dragStart.press(forDuration: 1, thenDragTo: dragFinish, withVelocity: .slow, thenHoldForDuration: 1)

        var previous: [CGRect]?
        let paired = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in
            guard self.app.state == .runningForeground, settings.windows.firstMatch.exists else {
                previous = nil
                return false
            }
            let frames = [self.app.windows.firstMatch.frame, self.app.webViews.firstMatch.frame,
                settings.windows.firstMatch.frame]
            let equal = previous == frames
            previous = frames
            return equal && !frames[0].isEmpty && !frames[1].isEmpty && !frames[2].isEmpty
                && frames[0].intersection(frames[2]).isEmpty
                && abs(frames[0].width - beforeWindow.width) > 80
                && abs(frames[1].width - beforeContent.width) > 80
        }, object: nil)
        let pairedResult = XCTWaiter.wait(for: [paired], timeout: 10)
        capture("ipad-dock-paired-after-drop")
        capture("ipad-dock-paired-after-drop-settings", application: settings)
        capture("ipad-dock-paired-after-drop-springboard", application: springboard)
        XCTAssertEqual(pairedResult, .completed, "The actual Dock drop must settle two nonoverlapping App windows")
        XCTAssertEqual(processMarker(), process)
        XCTAssertEqual(name.value as? String, draft)
        name.tap() // Explicit focus restoration; this does not claim the original caret survived the OS drag.
        assertWindowedEditing(name, draft: draft, process: process, stage: "ipad-dock-paired-restored-keyboard")
        let window = app.windows.firstMatch.frame
        let settingsWindow = settings.windows.firstMatch.frame
        let finalScreen = screenElement.frame
        let geometry = XCTAttachment(string: "process=\(processMarker()); beforeWindow=\(beforeWindow); beforeContent=\(beforeContent); window=\(window); content=\(app.webViews.firstMatch.frame); settingsWindow=\(settingsWindow); screen=\(finalScreen); initialNameHeight=\(initialNameHeight); name=\(name.frame)")
        geometry.name = "ipad-dock-paired-final-native-geometry"
        geometry.lifetime = .keepAlways
        add(geometry)
        capture("ipad-dock-paired-final-settings", application: settings)
        capture("ipad-dock-paired-final-springboard", application: springboard)
        XCTAssertGreaterThan(finalScreen.width, 0)
        XCTAssertTrue(finalScreen.contains(window) && finalScreen.contains(settingsWindow))
        XCTAssertFalse(window.isEmpty || settingsWindow.isEmpty)
        XCTAssertTrue(window.intersection(settingsWindow).isEmpty, "Overlapping windows are not side-by-side tiling")
        XCTAssertLessThan(window.width, finalScreen.width * 0.8)
        XCTAssertLessThan(settingsWindow.width, finalScreen.width * 0.8)
        XCTAssertEqual(window.minY, settingsWindow.minY, accuracy: 1)
        XCTAssertEqual(window.height, settingsWindow.height, accuracy: 1)
        XCTAssertGreaterThanOrEqual(window.height, finalScreen.height * 0.65)
        XCTAssertGreaterThan(abs(window.width - beforeWindow.width), 80)
        XCTAssertGreaterThan(abs(app.webViews.firstMatch.frame.width - beforeContent.width), 80)
        XCTAssertGreaterThanOrEqual(name.frame.height, initialNameHeight - 1)
        XCTAssertTrue(settings.staticTexts.allElementsBoundByIndex.contains {
            !$0.frame.isEmpty && settingsWindow.contains($0.frame) && $0.isHittable
        }, "Settings must have actual reachable content in its second window")

        let suffix = " [Dock pair continued]"
        XCTAssertFalse(draft.contains(suffix))
        name.typeText(suffix)
        let continued = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in
            guard let value = name.value as? String else { return false }
            return value.components(separatedBy: suffix).count == 2
                && value.replacingOccurrences(of: suffix, with: "") == draft
        }, object: nil)
        let continuedResult = XCTWaiter.wait(for: [continued], timeout: 5)
        capture("ipad-dock-paired-after-continued-input")
        XCTAssertEqual(continuedResult, .completed, "Continued input must add exactly one suffix to the complete original draft")
        let continuedDraft = name.value as? String
        assertWindowedEditing(name, draft: continuedDraft, process: process, stage: "ipad-dock-paired-continued-keyboard")
        capture("ipad-dock-paired-continued-settings", application: settings)
        capture("ipad-dock-paired-continued-springboard", application: springboard)
        app.terminate() // Discard this unsaved synthetic draft; OS restoration remains with the operator.
    }

    /// Actual top-bar Left/Fill presets, preserving editing without hiding the keyboard for a corner drag.
    func testIPadWindowPresetsKeepsEditingDraft() throws {
        XCTAssertEqual(UIDevice.current.userInterfaceIdiom, .pad)
        let name = openEditingDraft()
        name.tap()
        var insertionToken = " Window preset draft"
        name.typeText(insertionToken)
        var draft = (name.value as? String) ?? ""
        XCTAssertEqual(draft.components(separatedBy: insertionToken).count, 2,
            "The initial synthetic token must occur exactly once at the actual insertion point")
        let process = processMarker()
        assertWindowedEditing(name, draft: draft, process: process, stage: "ipad-preset-initial")
        let initialNameHeight = name.frame.height
        for (stage, imageIdentifier, smaller) in [
            ("ipad-preset-left", "inset.filled.lefthalf.rectangle", true),
            ("ipad-preset-fill", "rectangle.inset.filled", false),
        ] {
            let beforeWindow = app.windows.firstMatch.frame
            let beforeContent = app.webViews.firstMatch.frame
            choosePolarisWindowLayout(imageIdentifier, stage: stage)
            let resized = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in
                guard self.app.state == .runningForeground else { return false }
                let direction: CGFloat = smaller ? -1 : 1
                return direction * (self.app.windows.firstMatch.frame.width - beforeWindow.width) > 80
                    && direction * (self.app.webViews.firstMatch.frame.width - beforeContent.width) > 80
            }, object: nil)
            let resizedResult = XCTWaiter.wait(for: [resized], timeout: 10)
            capture(stage + "-after-os-layout")
            capture(stage + "-after-os-layout-springboard",
                application: XCUIApplication(bundleIdentifier: "com.apple.springboard"))
            XCTAssertEqual(resizedResult, .completed, "The real preset must resize both native Window and WebView")
            XCTAssertEqual(processMarker(), process)
            XCTAssertEqual(name.value as? String, draft)
            // If the OS hides the keyboard or drops actual editing, this strict gate fails;
            // no Hide, Name tap, App activation/relaunch or guessed Show action restores it.
            assertWindowedEditing(name, draft: draft, process: process, stage: stage + "-before-continuation")
            let suffix = smaller ? " [left continued]" : " [fill continued]"
            name.typeText(suffix)
            draft = draft.replacingOccurrences(of: insertionToken, with: insertionToken + suffix)
            insertionToken += suffix
            let continued = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in
                name.value as? String == draft
            }, object: nil)
            let continuedResult = XCTWaiter.wait(for: [continued], timeout: 5)
            capture(stage + "-after-continued-input")
            XCTAssertEqual(continuedResult, .completed, "The original focused insertion point must accept continued typing")
            assertWindowedEditing(name, draft: draft, process: process, stage: stage + "-continued-keyboard")
            let direction: CGFloat = smaller ? -1 : 1
            XCTAssertGreaterThan(direction * (app.windows.firstMatch.frame.width - beforeWindow.width), 80)
            XCTAssertGreaterThan(direction * (app.webViews.firstMatch.frame.width - beforeContent.width), 80)
            XCTAssertGreaterThanOrEqual(name.frame.height, initialNameHeight - 1)
            let geometry = XCTAttachment(string: "process=\(processMarker()); beforeWindow=\(beforeWindow); beforeContent=\(beforeContent); window=\(app.windows.firstMatch.frame); content=\(app.webViews.firstMatch.frame); initialNameHeight=\(initialNameHeight); name=\(name.frame)")
            geometry.name = stage + "-final-native-geometry"
            geometry.lifetime = .keepAlways
            add(geometry)
        }
        app.terminate() // Discard the synthetic draft only; never tap Save or connect a VPN.
    }

    private func navigation(_ index: Int) -> XCUIElement {
        app.buttons.matching(NSPredicate(format: "label IN %@", destinations[index])).firstMatch
    }

    private func tailscaleCreationOption() -> XCUIElement {
        // Match the chooser hint too: an existing node can also start with Tailscale.
        app.buttons.matching(NSPredicate(
            format: "label BEGINSWITH %@ AND (label CONTAINS %@ OR label CONTAINS %@ OR label CONTAINS %@)",
            "Tailscale", "Interactive login or Auth Key", "交互登录或使用 Auth Key", "互動登入或使用 Auth Key"
        )).firstMatch
    }

    private func capture(_ name: String, application: XCUIApplication? = nil) {
        let attachment = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
        let hierarchy = XCTAttachment(string: (application ?? app).debugDescription)
        hierarchy.name = name + "-accessibility"
        hierarchy.lifetime = .keepAlways
        add(hierarchy)
    }

    private func assertNavigationInsideWindow() {
        let window = app.windows.firstMatch.frame
        XCTAssertGreaterThan(window.width, 0)
        for index in destinations.indices {
            let button = navigation(index)
            XCTAssertTrue(button.exists, "Missing navigation: \(destinations[index])")
            XCTAssertTrue(button.isHittable, "Unreachable navigation: \(destinations[index])")
            XCTAssertTrue(window.insetBy(dx: -1, dy: -1).contains(button.frame), "Navigation outside window: \(button.frame), \(window)")
        }
        let first = navigation(0).frame
        let last = navigation(4).frame
        if UIDevice.current.userInterfaceIdiom == .pad || window.width > window.height {
            XCTAssertEqual(first.midX, last.midX, accuracy: 4, "Wide view needs a vertical navigation rail")
            XCTAssertGreaterThan(last.midY - first.midY, 120)
        } else {
            XCTAssertEqual(first.midY, last.midY, accuracy: 4, "Phone portrait needs bottom navigation")
            XCTAssertGreaterThan(last.midX - first.midX, 120)
            XCTAssertGreaterThan(first.midY, window.minY + window.height * 0.65)
        }
    }

    func testAllDestinationsAndLandscape() throws {
        for index in destinations.indices {
            let button = navigation(index)
            XCTAssertTrue(button.waitForExistence(timeout: 10))
            button.tap()
            if index == 1 {
                let add = app.buttons.matching(NSPredicate(format: "label IN %@", ["Add", "添加", "新增"])).firstMatch
                XCTAssertTrue(add.waitForExistence(timeout: 10), "Nodes content did not open")
            }
            assertNavigationInsideWindow()
            capture("portrait-\(index)")
        }
        navigation(0).tap()
        XCUIDevice.shared.orientation = .landscapeLeft
        XCTAssertTrue(navigation(0).waitForExistence(timeout: 10))
        assertNavigationInsideWindow()
        capture("landscape-home")
        XCUIDevice.shared.orientation = .portrait
    }

    func testRelaunchRetainsUsableNavigation() throws {
        navigation(1).tap()
        app.terminate()
        app.launch()
        XCTAssertTrue(navigation(0).waitForExistence(timeout: 30))
        assertNavigationInsideWindow()
        navigation(0).tap()
        capture("relaunched-home")
    }

    /// Ordinary iPhone/iPad rotation only; this is not Duo pose acceptance.
    func testRotationKeepsEditingDraft() throws {
        try exerciseEditingDraft(allowKeyboardDismissal: false)
    }

    /// Run only with simctl content_size set to the maximum accessibility size.
    func testAccessibilityRotationKeepsEditingDraft() throws {
        try exerciseEditingDraft(allowKeyboardDismissal: true)
    }

    /// Optional setup/inspection on the dedicated iPad only; this changes its OS window mode.
    func testInspectIPadMultitaskingSettings() throws {
        XCTAssertEqual(UIDevice.current.userInterfaceIdiom, .pad)
        let settings = XCUIApplication(bundleIdentifier: "com.apple.Preferences")
        settings.launch()
        capture("ipad-settings-initial", application: settings)
        capture("ipad-settings-foreground-springboard", application: XCUIApplication(bundleIdentifier: "com.apple.springboard"))
        let multitasking = settings.staticTexts.matching(NSPredicate(format: "label IN %@",
            ["Multitasking & Gestures", "多任务与手势", "多工處理與手勢"])).firstMatch
        XCTAssertTrue(multitasking.waitForExistence(timeout: 10), settings.debugDescription)
        multitasking.tap()
        capture("ipad-multitasking-options", application: settings)
        let windowed = settings.buttons["com.apple.settings.multitaskingAndGestures.windowedApps"]
        let appeared = windowed.waitForExistence(timeout: 5)
        capture("ipad-windowed-apps-control", application: settings)
        XCTAssertTrue(appeared && windowed.isHittable, settings.debugDescription)
        if !windowed.isSelected { windowed.tap() }
        let selected = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in windowed.isSelected }, object: nil)
        let result = XCTWaiter.wait(for: [selected], timeout: 5)
        capture("ipad-windowed-apps-selected", application: settings)
        XCTAssertEqual(result, .completed, "Windowed Apps must actually be selected; this is setup only")
    }

    /// Discover Settings' own native Window submenu; no window layout is chosen.
    func testInspectIPadSettingsWindowMenu() throws {
        XCTAssertEqual(UIDevice.current.userInterfaceIdiom, .pad)
        let settings = XCUIApplication(bundleIdentifier: "com.apple.Preferences")
        settings.launch()
        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        capture("ipad-settings-window-menu-initial", application: settings)
        capture("ipad-settings-window-menu-initial-springboard", application: springboard)
        XCTAssertEqual(settings.state, .runningForeground)
        func settingsProcessMarker() -> String {
            let header = settings.debugDescription.components(separatedBy: "\n").first ?? ""
            let marker = header.components(separatedBy: ",").first { $0.contains("pid:") } ?? ""
            XCTAssertFalse(marker.isEmpty, "The actual Settings AX process identifier is required")
            return marker.trimmingCharacters(in: .whitespaces)
        }
        let process = settingsProcessMarker()
        let appName = springboard.statusBars.staticTexts.matching(
            NSPredicate(format: "label == %@", "设置")).firstMatch
        let nameAppeared = appName.waitForExistence(timeout: 5)
        capture("ipad-settings-window-menu-before-status-bar", application: springboard)
        XCTAssertTrue(nameAppeared && appName.isHittable, "The observed Settings status-bar name must be reachable")
        appName.tap()
        let settingsMenu = springboard.windows.containing(.button, identifier: "设置").firstMatch
        let menuAppeared = settingsMenu.waitForExistence(timeout: 5)
        capture("ipad-settings-window-menu-revealed", application: springboard)
        XCTAssertTrue(menuAppeared, "The menu must belong to the actual Settings App-name button")
        let windowMenu = settingsMenu.buttons.matching(NSPredicate(format: "label == %@", "窗口")).firstMatch
        let windowAppeared = windowMenu.waitForExistence(timeout: 5)
        capture("ipad-settings-window-menu-before-open", application: springboard)
        XCTAssertTrue(windowAppeared && windowMenu.isEnabled && windowMenu.isHittable,
            "The actual Settings Window menu button must be enabled and reachable")
        windowMenu.tap()
        capture("ipad-settings-window-menu-popup", application: springboard)
        capture("ipad-settings-window-menu-popup-settings", application: settings)
        let moveResize = settingsMenu.buttons.matching(NSPredicate(format: "label == %@", "移动与调整大小")).firstMatch
        let moveResizeAppeared = moveResize.waitForExistence(timeout: 5)
        capture("ipad-settings-window-menu-before-move-resize", application: springboard)
        XCTAssertTrue(moveResizeAppeared && moveResize.isEnabled && moveResize.isHittable,
            "The actual Settings Move & Resize button must be enabled and reachable")
        moveResize.tap()
        capture("ipad-settings-window-move-resize-submenu", application: springboard)
        capture("ipad-settings-window-move-resize-submenu-settings", application: settings)
        XCTAssertEqual(settings.state, .runningForeground)
        XCTAssertEqual(settingsProcessMarker(), process)
        // Read the real submenu before adding any Settings layout action; do not activate Polaris.
    }

    /// Real OS corner drags in Windowed Apps/Stage Manager, not legacy Split View or an injected viewport.
    func testIPadWindowResizeKeepsEditingDraft() throws {
        XCTAssertEqual(UIDevice.current.userInterfaceIdiom, .pad)
        // Keep OS handle coordinates in portrait for this first windowing case. Rotation cases
        // exercise landscape separately; a portrait window PASS is not landscape tiling acceptance.
        XCUIDevice.shared.orientation = .portrait
        var previousWindowFrames: [CGRect]?
        let readyWindow = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in
            guard self.app.state == .runningForeground else { previousWindowFrames = nil; return false }
            let frames = [self.app.windows.firstMatch.frame, self.app.webViews.firstMatch.frame]
            let stable = previousWindowFrames == frames
            previousWindowFrames = frames
            return stable && !frames[0].isEmpty && !frames[1].isEmpty
        }, object: nil)
        let readyResult = XCTWaiter.wait(for: [readyWindow], timeout: 10)
        capture("ipad-window-portrait-ready")
        XCTAssertEqual(readyResult, .completed, "The foreground native window must settle before editing")
        let name = openEditingDraft()
        name.tap()
        var insertionToken = " Window resize draft"
        name.typeText(insertionToken)
        var draft = (name.value as? String) ?? ""
        XCTAssertEqual(draft.components(separatedBy: insertionToken).count, 2,
            "The known inserted token must occur exactly once in the original Name value")
        let process = processMarker()
        let initialWindow = app.windows.firstMatch.frame
        let initialContent = app.webViews.firstMatch.frame
        assertWindowedEditing(name, draft: draft, process: process, stage: "ipad-window-initial")

        for (stage, smaller) in [("ipad-window-narrow", true), ("ipad-window-expanded", false)] {
            // The system resize handle is behind the docked keyboard. Dismiss via its real control,
            // keep this draft/scene and insertion point alive, drag the OS handle, then continue typing.
            let hide = app.buttons.matching(NSPredicate(format: "label IN %@",
                ["Hide keyboard", "隐藏键盘", "隱藏鍵盤", "Done", "完成"])).firstMatch
            XCTAssertTrue(hide.waitForExistence(timeout: 5) && hide.isHittable,
                "A real keyboard dismissal control is required to reach the OS resize handle")
            hide.tap()
            XCTAssertTrue(app.keyboards.firstMatch.waitForNonExistence(timeout: 10))
            let beforeWindow = app.windows.firstMatch.frame
            let beforeContent = app.webViews.firstMatch.frame
            capture(stage + "-before-os-drag")
            let windowElement = app.windows.firstMatch
            let origin = windowElement.coordinate(withNormalizedOffset: .zero)
            let start = windowElement.coordinate(withNormalizedOffset: CGVector(
                dx: (beforeWindow.width - 6) / beforeWindow.width,
                dy: (beforeWindow.height - 6) / beforeWindow.height))
            let targetWidth = smaller ? initialWindow.width * 0.65 : initialWindow.width
            let targetHeight = smaller ? initialWindow.height * 0.82 : initialWindow.height
            let finish = windowElement.coordinate(withNormalizedOffset: CGVector(
                dx: min(initialWindow.maxX - beforeWindow.minX - 6, targetWidth - 6) / beforeWindow.width,
                dy: min(initialWindow.maxY - beforeWindow.minY - 6, targetHeight - 6) / beforeWindow.height))
            let dragGeometry = XCTAttachment(string: "initialWindow=\(initialWindow); initialContent=\(initialContent); beforeWindow=\(beforeWindow); beforeContent=\(beforeContent); origin.screenPoint=\(origin.screenPoint); dragStart=\(start.screenPoint); dragFinish=\(finish.screenPoint)")
            dragGeometry.name = stage + "-os-drag-coordinates"
            dragGeometry.lifetime = .keepAlways
            add(dragGeometry)
            start.press(forDuration: 0.2, thenDragTo: finish)
            let resized = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in
                guard self.app.state == .runningForeground else { return false }
                let window = self.app.windows.firstMatch.frame
                let content = self.app.webViews.firstMatch.frame
                let direction: CGFloat = smaller ? -1 : 1
                return direction * (window.width - beforeWindow.width) > 80
                    && direction * (content.width - beforeContent.width) > 80
            }, object: nil)
            let result = XCTWaiter.wait(for: [resized], timeout: 10)
            capture(stage + "-after-os-drag")
            XCTAssertEqual(result, .completed,
                "OS window and native WebView must both resize: before=\(beforeWindow)/\(beforeContent), after=\(app.windows.firstMatch.frame)/\(app.webViews.firstMatch.frame)")
            XCTAssertEqual(processMarker(), process, "OS resizing relaunched the App")
            XCTAssertEqual(app.state, .runningForeground, "AppSwitcher previews are not window resize evidence")
            XCTAssertEqual(name.value as? String, draft, "OS resizing lost the unsaved draft")
            XCTAssertTrue(name.debugDescription.contains("Keyboard Focused"), "OS resizing lost the original insertion point")
            let suffix = smaller ? " [narrow continued]" : " [expanded continued]"
            name.typeText(suffix)
            // Tapping initially may insert before the default name. Preserve that actual insertion point,
            // rather than assume the caret is at the end of the complete field value.
            draft = draft.replacingOccurrences(of: insertionToken, with: insertionToken + suffix)
            insertionToken += suffix
            let continued = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in
                name.value as? String == draft
            }, object: nil)
            let continuedResult = XCTWaiter.wait(for: [continued], timeout: 5)
            capture(stage + "-after-continued-input")
            XCTAssertEqual(continuedResult, .completed, "OS resizing lost continued input at the original insertion point")
            assertWindowedEditing(name, draft: draft, process: process, stage: stage + "-keyboard")
            // A transient resize does not count if the OS snaps back while keyboard/layout settles.
            let direction: CGFloat = smaller ? -1 : 1
            XCTAssertGreaterThan(direction * (app.windows.firstMatch.frame.width - beforeWindow.width), 80)
            XCTAssertGreaterThan(direction * (app.webViews.firstMatch.frame.width - beforeContent.width), 80)
        }
        XCTAssertGreaterThan(initialContent.width, 0)
        app.terminate() // Discard only this in-memory draft; never save or start a VPN.
    }

    private func processMarker() -> String {
        let header = app.debugDescription.components(separatedBy: "\n").first ?? ""
        let marker = header.components(separatedBy: ",").first { $0.contains("pid:") } ?? ""
        XCTAssertFalse(marker.isEmpty, "The App AX process identifier must be present")
        return marker.trimmingCharacters(in: .whitespaces)
    }

    private var hasVisibleSoftwareKeyboard: Bool {
        let keyboard = app.keyboards.firstMatch
        return keyboard.exists && keyboard.frame.height > 0
            && !app.windows.firstMatch.frame.intersection(keyboard.frame).isEmpty
    }

    private func keyboardAccessoryFrames() -> [CGRect] {
        let window = app.windows.firstMatch.frame
        let others = ["SystemInputAssistantView", "inputView"].flatMap {
            app.otherElements.matching(identifier: $0).allElementsBoundByIndex
        }
        let toolbars = ["完成", "Done"].flatMap {
            app.toolbars.containing(.button, identifier: $0).allElementsBoundByIndex
        }
        return (others + toolbars).map { $0.frame.intersection(window) }
            .filter { !$0.isEmpty && !$0.isNull }
            .sorted { ($0.minY, $0.minX, $0.height, $0.width) < ($1.minY, $1.minX, $1.height, $1.width) }
    }

    private func assertWindowedEditing(_ name: XCUIElement, draft: String?, process: String, stage: String) {
        capture(stage) // Preserve the actual image/AX before any geometry assertion can stop this test.
        waitForEditingGeometry(name, stage: stage + "-settled", requireKeyboard: true)
        XCTAssertEqual(app.state, .runningForeground)
        let window = app.windows.firstMatch.frame
        let content = window.intersection(app.webViews.firstMatch.frame)
        let keyboard = app.keyboards.firstMatch
        let save = app.buttons.matching(NSPredicate(format: "label IN %@", ["Save", "保存", "儲存"])).firstMatch
        let title = app.staticTexts.matching(NSPredicate(format: "label IN %@",
            ["Connect Tailscale", "连接 Tailscale", "連接 Tailscale"])).firstMatch
        let accessoryFrames = keyboardAccessoryFrames()
        let visibleBottom = min(content.maxY, min(keyboard.frame.minY,
            accessoryFrames.map { $0.minY }.min() ?? keyboard.frame.minY))
        let geometry = XCTAttachment(string: "process=\(processMarker()); window=\(window); content=\(content); name=\(name.frame); save=\(save.frame); title=\(title.frame); keyboard=\(keyboard.frame); accessories=\(accessoryFrames)")
        geometry.name = stage + "-native-geometry"
        geometry.lifetime = .keepAlways
        add(geometry)
        XCTAssertEqual(processMarker(), process)
        XCTAssertEqual(name.value as? String, draft)
        XCTAssertTrue(name.isHittable && save.isHittable)
        XCTAssertTrue(name.debugDescription.contains("Keyboard Focused"), "The original name field must own keyboard focus")
        XCTAssertGreaterThan(keyboard.frame.height, 0)
        XCTAssertFalse(window.intersection(keyboard.frame).isEmpty, "The actual keyboard must intersect the App window")
        // m-form-input uses a 2px outline with offset -2px: its focus border is inside this frame.
        XCTAssertTrue(content.contains(name.frame), "Focused field/border is clipped at the native content edge")
        XCTAssertTrue(content.contains(save.frame))
        XCTAssertTrue(title.exists)
        XCTAssertFalse(name.frame.intersects(title.frame), "Sheet heading covers the focused field")
        XCTAssertLessThanOrEqual(name.frame.maxY, visibleBottom + 1, "Keyboard/accessory covers Name")
        XCTAssertLessThanOrEqual(save.frame.maxY, visibleBottom + 1, "Keyboard/accessory covers Save")
        // UIKit traits can select bottom navigation in a narrow iPad window. Do not reuse the
        // fullscreen-only navigation assertion that treats every iPad as a vertical rail.
    }

    private func waitForEditingGeometry(_ name: XCUIElement, stage: String, landscape: Bool? = nil, requireKeyboard: Bool = false,
        footer: XCUIElement? = nil, headingLabels: [String] = ["Connect Tailscale", "连接 Tailscale", "連接 Tailscale"]) {
        let save = footer ?? app.buttons.matching(NSPredicate(format: "label IN %@", ["Save", "保存", "儲存"])).firstMatch
        if requireKeyboard {
            let visible = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in
                self.app.state == .runningForeground && self.hasVisibleSoftwareKeyboard
            }, object: nil)
            let result = XCTWaiter.wait(for: [visible], timeout: 10)
            capture(stage + "-positive-keyboard-wait")
            XCTAssertEqual(result, .completed, "A nonzero software keyboard must actually intersect the App window")
        }
        var previous: [CGRect]?
        let stable = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in
            guard self.app.state == .runningForeground, name.exists, save.exists,
                self.app.webViews.firstMatch.exists else { previous = nil; return false }
            let window = self.app.windows.firstMatch.frame
            if let landscape = landscape, (window.width > window.height) != landscape { previous = nil; return false }
            if requireKeyboard && !self.hasVisibleSoftwareKeyboard { previous = nil; return false }
            let keyboard = self.app.keyboards.firstMatch
            let title = self.app.staticTexts.matching(NSPredicate(format: "label IN %@", headingLabels)).firstMatch
            let frames = [window, self.app.webViews.firstMatch.frame, name.frame, save.frame,
                keyboard.exists ? keyboard.frame : .zero, title.exists ? title.frame : .zero]
                + self.keyboardAccessoryFrames()
            let equal = previous == frames
            previous = frames
            return equal && !window.isEmpty
        }, object: nil)
        let result = XCTWaiter.wait(for: [stable], timeout: 10)
        capture(stage)
        XCTAssertEqual(result, .completed, "Native window/field/footer/keyboard geometry never settled")
    }

    private func openEditingDraft() -> XCUIElement {
        navigation(1).tap()
        let add = app.buttons.matching(NSPredicate(format: "label IN %@", ["Add", "添加", "新增"])).firstMatch
        XCTAssertTrue(add.waitForExistence(timeout: 10))
        add.tap()
        let mesh = app.buttons.matching(NSPredicate(format: "label IN %@", ["Add mesh access", "添加组网接入", "新增組網接入"])).firstMatch
        XCTAssertTrue(mesh.waitForExistence(timeout: 10))
        mesh.tap()
        let tailscale = tailscaleCreationOption()
        XCTAssertTrue(tailscale.waitForExistence(timeout: 10))
        tailscale.tap()
        let name = app.textFields.matching(NSPredicate(format: "label BEGINSWITH %@ OR label BEGINSWITH %@ OR label BEGINSWITH %@", "Node name", "节点名称", "節點名稱")).firstMatch
        XCTAssertTrue(name.waitForExistence(timeout: 10))
        return name
    }

    private func exerciseEditingDraft(allowKeyboardDismissal: Bool) throws {
        let name = openEditingDraft()
        name.tap()
        name.typeText(" Continuity draft")
        let draftValue = name.value as? String
        XCTAssertTrue(draftValue?.contains("Continuity draft") == true)
        for orientation: UIDeviceOrientation in [.landscapeLeft, .portrait] {
            XCUIDevice.shared.orientation = orientation
            waitForEditingGeometry(name, stage: "editing-draft-\(orientation.rawValue)-rotation-settled",
                landscape: orientation == .landscapeLeft)
            // The separate maximum-text case may have dismissed the keyboard in the preceding pose.
            if !hasVisibleSoftwareKeyboard { name.tap() }
            capture("editing-draft-\(orientation.rawValue)-before-geometry-asserts")
            waitForEditingGeometry(name, stage: "editing-draft-\(orientation.rawValue)-keyboard-settled",
                landscape: orientation == .landscapeLeft, requireKeyboard: true)
            XCTAssertTrue(name.waitForExistence(timeout: 10))
            XCTAssertEqual(name.value as? String, draftValue)
            XCTAssertTrue(name.isHittable, "Editing field became unreachable after rotation")
            let title = app.staticTexts.matching(NSPredicate(format: "label IN %@", ["Connect Tailscale", "连接 Tailscale", "連接 Tailscale"])).firstMatch
            XCTAssertTrue(title.exists, "The actual sheet heading is missing")
            let content = app.windows.firstMatch.frame.intersection(app.webViews.firstMatch.frame)
            // The production Name focus outline is inset, not an outer 2pt ring.
            XCTAssertTrue(content.contains(name.frame), "The focused field/border is clipped at the native content edge")
            if !title.frame.isEmpty {
                XCTAssertFalse(name.frame.intersects(title.frame),
                    "The sheet title covers the focused field despite accessibility hittability: field=\(name.frame), title=\(title.frame)")
            }
            let keyboard = app.keyboards.firstMatch
            XCTAssertGreaterThan(keyboard.frame.height, 0)
            do {
                let accessoryTop = keyboardAccessoryFrames().map { $0.minY }.min()
                let visibleBottom = min(keyboard.frame.minY, accessoryTop ?? keyboard.frame.minY)
                XCTAssertLessThanOrEqual(name.frame.maxY, visibleBottom + 1,
                    "Keyboard/accessory overlaps the editing field: \(name.frame), bottom=\(visibleBottom)")
                let save = app.buttons.matching(NSPredicate(format: "label IN %@", ["Save", "保存", "儲存"])).firstMatch
                if allowKeyboardDismissal && (!save.isHittable || save.frame.maxY > visibleBottom + 1
                    || !content.contains(save.frame)) {
                    // With maximum accessibility text in a short landscape
                    // viewport, the field and footer cannot both fit. Preserve
                    // the draft while using the system's keyboard dismissal.
                    capture("editing-draft-\(orientation.rawValue)-before-keyboard-dismissal")
                    let done = app.buttons.matching(NSPredicate(format: "label IN %@",
                        ["Done", "完成", "Hide keyboard", "隐藏键盘", "隱藏鍵盤"])).firstMatch
                    XCTAssertTrue(done.isHittable, "Neither Save nor the keyboard dismissal is reachable")
                    done.tap()
                    XCTAssertTrue(keyboard.waitForNonExistence(timeout: 10))
                    waitForEditingGeometry(name, stage: "editing-draft-\(orientation.rawValue)-dismissed-settled",
                        landscape: orientation == .landscapeLeft)
                    XCTAssertEqual(name.value as? String, draftValue)
                    XCTAssertTrue(save.isHittable, "Save is still unreachable after keyboard dismissal")
                    XCTAssertTrue(app.windows.firstMatch.frame.intersection(app.webViews.firstMatch.frame).contains(save.frame))
                    capture("editing-draft-\(orientation.rawValue)-after-keyboard-dismissal")
                    continue
                }
                XCTAssertTrue(save.isHittable)
                XCTAssertTrue(content.contains(save.frame))
                XCTAssertLessThanOrEqual(save.frame.maxY, visibleBottom + 1,
                    "Keyboard/accessory overlaps Save: \(save.frame), bottom=\(visibleBottom)")
            }
            capture("editing-draft-\(orientation.rawValue)")
        }
        // No save or VPN action; discard this in-memory draft with the test host.
        app.terminate()
    }

    /// Pure multiline draft only: never invoke Parse, Import, file picking or Save.
    func testRotationKeepsMultilineDraft() throws {
        navigation(1).tap()
        let add = app.buttons.matching(NSPredicate(format: "label IN %@", ["Add", "添加", "新增"])).firstMatch
        XCTAssertTrue(add.waitForExistence(timeout: 10))
        add.tap()
        let manualImport = app.buttons.matching(NSPredicate(format: "label IN %@",
            ["Import nodes", "导入节点", "匯入節點"])).firstMatch
        XCTAssertTrue(manualImport.waitForExistence(timeout: 10))
        manualImport.tap()
        let text = app.textViews.matching(NSPredicate(format:
            "label BEGINSWITH %@ OR label BEGINSWITH %@ OR label BEGINSWITH %@",
            "Paste share link / subscription URL / Base64", "粘贴分享链接 / 订阅 URL / Base64", "貼上分享連結 / 訂閱 URL / Base64")).firstMatch
        XCTAssertTrue(text.waitForExistence(timeout: 10))
        var draft = (1...9).map { "layout-only draft line \($0)" }.joined(separator: "\n")
        text.tap()
        text.typeText(draft)
        XCTAssertEqual(text.value as? String, draft)
        let process = processMarker()
        let parse = app.buttons.matching(NSPredicate(format: "label IN %@", ["Parse", "解析"])).firstMatch
        let headingLabels = ["Import Nodes", "导入节点", "匯入節點"]
        for orientation: UIDeviceOrientation in [.landscapeLeft, .portrait] {
            XCUIDevice.shared.orientation = orientation
            let stage = "multiline-draft-\(orientation.rawValue)"
            waitForEditingGeometry(text, stage: stage + "-rotation-settled",
                landscape: orientation == .landscapeLeft, footer: parse, headingLabels: headingLabels)
            let keyboard = app.keyboards.firstMatch
            capture(stage + "-before-geometry-asserts")
            waitForEditingGeometry(text, stage: stage + "-keyboard-settled",
                landscape: orientation == .landscapeLeft, requireKeyboard: true, footer: parse, headingLabels: headingLabels)
            XCTAssertEqual(processMarker(), process)
            XCTAssertEqual(text.value as? String, draft)
            XCTAssertTrue(text.debugDescription.contains("Keyboard Focused"))
            XCTAssertGreaterThan(keyboard.frame.height, 0)
            // Continue at the original insertion point without tapping, selecting or replacing text.
            let suffix = orientation == .landscapeLeft ? " [landscape continued]" : " [portrait continued]"
            text.typeText(suffix)
            draft += suffix
            let continued = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in
                text.value as? String == draft
            }, object: nil)
            let continuedResult = XCTWaiter.wait(for: [continued], timeout: 5)
            capture(stage + "-after-continued-input")
            XCTAssertEqual(continuedResult, .completed, "Rotation lost the original insertion point or continued input")
            waitForEditingGeometry(text, stage: stage + "-continued-input-settled",
                landscape: orientation == .landscapeLeft, requireKeyboard: true, footer: parse, headingLabels: headingLabels)
            let content = app.windows.firstMatch.frame.intersection(app.webViews.firstMatch.frame)
            let title = app.staticTexts.matching(NSPredicate(format: "label IN %@", headingLabels)).firstMatch
            let assistantTop = keyboardAccessoryFrames().map { $0.minY }.min()
            let visibleBottom = min(content.maxY, min(keyboard.frame.minY, assistantTop ?? keyboard.frame.minY))
            XCTAssertEqual(processMarker(), process)
            XCTAssertEqual(text.value as? String, draft)
            XCTAssertTrue(text.isHittable && text.debugDescription.contains("Keyboard Focused"))
            XCTAssertGreaterThan(keyboard.frame.height, 0)
            XCTAssertFalse(app.windows.firstMatch.frame.intersection(keyboard.frame).isEmpty)
            // mif-text also has m-form-input: actual CSS is outline 2px / offset -2px.
            XCTAssertTrue(content.contains(text.frame), "The complete textarea/border is clipped")
            XCTAssertTrue(title.exists)
            XCTAssertFalse(text.frame.intersects(title.frame), "Sheet heading covers the multiline field")
            XCTAssertLessThanOrEqual(text.frame.maxY, visibleBottom + 1, "Keyboard/accessory covers the multiline field")
        }
        // Dismiss once after both continued edits; re-tapping after each dismissal can move the caret.
        let hide = app.buttons.matching(NSPredicate(format: "label IN %@",
            ["Done", "完成", "Hide keyboard", "隐藏键盘", "隱藏鍵盤"])).firstMatch
        XCTAssertTrue(hide.isHittable, "A real dismissal control must make the untouched Parse action reachable")
        hide.tap()
        let dismissed = app.keyboards.firstMatch.waitForNonExistence(timeout: 10)
        capture("multiline-draft-after-keyboard-dismissal")
        XCTAssertTrue(dismissed)
        waitForEditingGeometry(text, stage: "multiline-draft-dismissed-settled",
            landscape: false, footer: parse, headingLabels: headingLabels)
        XCTAssertEqual(processMarker(), process)
        XCTAssertEqual(text.value as? String, draft)
        XCTAssertTrue(parse.isHittable)
        XCTAssertTrue(app.windows.firstMatch.frame.intersection(app.webViews.firstMatch.frame).contains(parse.frame))
        app.terminate() // Discard the synthetic unsaved text; no parser or import request.
    }

    func testTsConfigurationSaveWithoutLogin() throws {
        navigation(1).tap()
        let add = app.buttons.matching(NSPredicate(format: "label IN %@", ["Add", "添加", "新增"])).firstMatch
        XCTAssertTrue(add.waitForExistence(timeout: 10))
        add.tap()
        let mesh = app.buttons.matching(NSPredicate(format: "label IN %@", ["Add mesh access", "添加组网接入", "新增組網接入"])).firstMatch
        XCTAssertTrue(mesh.waitForExistence(timeout: 10), app.debugDescription)
        mesh.tap()
        let tailscale = tailscaleCreationOption()
        XCTAssertTrue(tailscale.waitForExistence(timeout: 10), app.debugDescription)
        tailscale.tap()
        let name = app.textFields.matching(NSPredicate(format: "label BEGINSWITH %@ OR label BEGINSWITH %@ OR label BEGINSWITH %@", "Node name", "节点名称", "節點名稱")).firstMatch
        XCTAssertTrue(name.waitForExistence(timeout: 10), app.debugDescription)
        XCTAssertTrue(name.isHittable)
        let save = app.buttons.matching(NSPredicate(format: "label IN %@", ["Save", "保存", "儲存"])).firstMatch
        XCTAssertTrue(save.isHittable)
        capture("ts-before-save")
        save.tap()
        let saved = app.staticTexts.matching(NSPredicate(format: "label BEGINSWITH %@ OR label BEGINSWITH %@ OR label BEGINSWITH %@", "Settings saved; login has not been confirmed.", "配置已保存，尚未确认登录。", "設定已儲存，尚未確認登入。")).firstMatch
        XCTAssertTrue(saved.waitForExistence(timeout: 15), app.debugDescription)
        capture("ts-saved-without-login")
        app.buttons.matching(NSPredicate(format: "label IN %@", ["Close", "关闭", "關閉"])).firstMatch.tap()
        XCTAssertTrue(navigation(0).waitForExistence(timeout: 10))
    }
}
