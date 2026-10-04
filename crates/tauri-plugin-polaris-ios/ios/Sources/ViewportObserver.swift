import Foundation
import UIKit
import WebKit
import os

private struct ReservedViewportRegion: Encodable, Equatable {
    let kind: String
    let x: Double
    let y: Double
    let width: Double
    let height: Double
}

private struct ViewportSnapshot: Encodable, Equatable {
    let x: Double
    let y: Double
    let width: Double
    let height: Double
    let top: Double
    let right: Double
    let bottom: Double
    let left: Double
    let horizontalSizeClass: String
    let verticalSizeClass: String
    let fontScale: Double
    let keyboardVisibleBottom: Double
    let verticalBarEdge: String
    let verticalBarSide: String
    let reservedRegions: [ReservedViewportRegion]
}

private final class KeyboardLayoutProbe: UIView {
    var didLayout: (() -> Void)?
    override func layoutSubviews() { super.layoutSubviews(); didLayout?() }
}

/// Signals belong to the supplied WebView's scene, never the application's
/// global view controller or a device model. iOS 27.1 signals come from UIKit;
/// earlier systems retain the same size-class layout without inferred hinges.
final class ViewportObserver: UIView {
    private weak var webview: WKWebView?
    private let keyboardProbe = KeyboardLayoutProbe()
    private var loadingObservation: NSKeyValueObservation?
    private var traitRegistration: (any UITraitChangeRegistration)?
    private var barTraitRegistration: (any UITraitChangeRegistration)?
    private var reservedRegions: [ReservedViewportRegion] = []
    private var regionBounds: CGRect?
    private weak var regionWindow: UIWindow?
    private var queued = false
    private var forceQueued = false
    private var inFlight = false
    private var pending: (ViewportSnapshot, UInt64)?
    private var delivered: ViewportSnapshot?
    private static var sequence: UInt64 = 0
    private var lifetime: UInt64 = 0
    private var detached = false
    private let logger = Logger(subsystem: "com.polaris.app", category: "Viewport")

    init(webview: WKWebView) {
        self.webview = webview
        super.init(frame: webview.bounds)
        autoresizingMask = [.flexibleWidth, .flexibleHeight]
        backgroundColor = .clear
        isOpaque = false
        isUserInteractionEnabled = false
        accessibilityElementsHidden = true
        // The guide belongs to this scene's observer. Hidden keyboards use the
        // view bottom; four safe insets are already reported independently.
        keyboardLayoutGuide.usesBottomSafeArea = false
        // A single full-width bottom limit models the docked keyboard only.
        // Floating keyboards need rectangle-aware placement, not this limit.
        keyboardLayoutGuide.followsUndockedKeyboard = false
        keyboardProbe.translatesAutoresizingMaskIntoConstraints = false
        keyboardProbe.isUserInteractionEnabled = false
        keyboardProbe.isOpaque = false
        addSubview(keyboardProbe)
        // A guide change need not resize our full WebView-sized observer.
        // This probe's height does change, ensuring a layout callback follows
        // the actual guide, including interactive keyboard movement.
        NSLayoutConstraint.activate([
            keyboardProbe.topAnchor.constraint(equalTo: topAnchor),
            keyboardProbe.leadingAnchor.constraint(equalTo: leadingAnchor),
            keyboardProbe.trailingAnchor.constraint(equalTo: trailingAnchor),
            keyboardProbe.bottomAnchor.constraint(equalTo: keyboardLayoutGuide.topAnchor),
        ])
        keyboardProbe.didLayout = { [weak self] in self?.refresh() }
    }
    required init?(coder: NSCoder) { nil }

    func observes(_ webview: WKWebView) -> Bool { self.webview === webview && !detached }

    override func layoutSubviews() {
        super.layoutSubviews()
        // Read during UIKit's observation-tracked layout, before asynchronous
        // JavaScript coalescing. Bounds need not change when a region changes.
        if #available(iOS 27.1, *), let webview {
            let bounds = webview.bounds
            reservedRegions = [UIView.ReservedRegion.Kind.division, .occlusion].flatMap { kind in
                webview.reservedRegions(kind: kind).compactMap { region in
                    guard region.isActive else { return nil }
                    // frame already includes UIKit's interaction margins.
                    // Keep zero-thickness division lines; they still partition.
                    let rect = region.frame.intersection(bounds)
                    guard !rect.isNull, !rect.isInfinite,
                          rect.minX.isFinite, rect.minY.isFinite,
                          rect.width.isFinite, rect.height.isFinite else { return nil }
                    return ReservedViewportRegion(kind: kind == .division ? "division" : "occlusion",
                        x: rect.minX - bounds.minX, y: rect.minY - bounds.minY,
                        width: rect.width, height: rect.height)
                }
            }
        } else { reservedRegions = [] }
        regionBounds = webview?.bounds
        regionWindow = webview?.window
        refresh()
    }
    override func safeAreaInsetsDidChange() { super.safeAreaInsetsDidChange(); refresh() }
    override func didMoveToWindow() {
        super.didMoveToWindow()
        if window == nil { releaseObservations(); return }
        guard !detached, let webview else { return }
        if traitRegistration == nil {
            traitRegistration = registerForTraitChanges([UITraitHorizontalSizeClass.self, UITraitVerticalSizeClass.self, UITraitPreferredContentSizeCategory.self, UITraitLayoutDirection.self]) {
                (view: ViewportObserver, _: UITraitCollection) in view.setNeedsLayout(); view.refresh()
            }
        }
        if #available(iOS 27.1, *), barTraitRegistration == nil {
            barTraitRegistration = registerForTraitChanges(UITraitCollection.systemTraitsAffectingVerticalBarEdge) {
                (view: ViewportObserver, _: UITraitCollection) in view.setNeedsLayout(); view.refresh()
            }
        }
        if loadingObservation == nil {
            // WKWebView.loading is explicitly KVO-compliant. This supplements
            // creation-time load() without replacing Tauri's navigation delegate.
            loadingObservation = webview.observe(\.isLoading, options: [.new]) { [weak self] _, change in
                guard change.newValue == false else { return }
                DispatchQueue.main.async { [weak self] in self?.refresh(force: true) }
            }
        }
        setNeedsLayout()
        refresh(force: true)
    }

    func detach() {
        detached = true
        releaseObservations()
        removeFromSuperview()
        webview = nil
    }
    private func releaseObservations() {
        loadingObservation?.invalidate()
        loadingObservation = nil
        if let traitRegistration { unregisterForTraitChanges(traitRegistration) }
        traitRegistration = nil
        if let barTraitRegistration { unregisterForTraitChanges(barTraitRegistration) }
        barTraitRegistration = nil
        reservedRegions = []
        regionBounds = nil
        regionWindow = nil
        lifetime += 1
        // A submitted evaluation cannot be cancelled by leaving its window.
        // Keep its physical slot until that call's actual callback returns.
        pending = nil
        delivered = nil
    }

    func refresh(force: Bool = false) {
        guard !detached else { return }
        forceQueued = forceQueued || force
        guard !queued else { return }
        queued = true
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.queued = false
            let force = self.forceQueued
            self.forceQueued = false
            guard !self.detached, let webview = self.webview, webview.window != nil,
                  webview.bounds.width > 0, webview.bounds.height > 0 else { return }
            // A trait/window callback can arrive before its scheduled layout.
            // Complete tracked capture first, then pair it with current bounds.
            if self.regionBounds != webview.bounds || self.regionWindow !== webview.window {
                self.setNeedsLayout()
            }
            self.layoutIfNeeded()
            guard self.regionBounds == webview.bounds,
                  self.regionWindow === webview.window else { return }
            let bounds = webview.bounds
            let insets = webview.safeAreaInsets
            let keyboardFrame = self.convert(self.keyboardLayoutGuide.layoutFrame, to: webview)
            let keyboardIntersection = bounds.intersection(keyboardFrame)
            let keyboardVisibleBottom = keyboardIntersection.isNull || keyboardIntersection.isEmpty
                ? bounds.height
                : max(0, min(bounds.height, keyboardIntersection.minY - bounds.minY))
            var barEdge = "unspecified", barSide = "unspecified"
            if #available(iOS 27.1, *) {
                switch webview.traitCollection.verticalBarEdge {
                case .leading: barEdge = "leading"
                case .trailing: barEdge = "trailing"
                default: break
                }
                if barEdge != "unspecified" {
                    let leadingIsLeft = webview.effectiveUserInterfaceLayoutDirection == .leftToRight
                    barSide = (barEdge == "leading") == leadingIsLeft ? "left" : "right"
                }
            }
            let snapshot = ViewportSnapshot(x: bounds.minX, y: bounds.minY, width: bounds.width, height: bounds.height,
                top: insets.top, right: insets.right, bottom: insets.bottom, left: insets.left,
                horizontalSizeClass: Self.sizeClass(webview.traitCollection.horizontalSizeClass),
                verticalSizeClass: Self.sizeClass(webview.traitCollection.verticalSizeClass),
                fontScale: UIFontMetrics(forTextStyle: .body).scaledValue(for: 16, compatibleWith: webview.traitCollection) / 16,
                keyboardVisibleBottom: keyboardVisibleBottom,
                verticalBarEdge: barEdge, verticalBarSide: barSide,
                reservedRegions: self.reservedRegions)
            if !force {
                if snapshot == self.pending?.0 { return }
                // Delivered A cannot suppress a return to A while B is in flight.
                if !self.inFlight && self.pending == nil && snapshot == self.delivered { return }
            }
            Self.sequence += 1
            self.pending = (snapshot, Self.sequence)
            self.deliver()
        }
    }

    private func deliver() {
        guard !inFlight, !detached, let webview, webview.window != nil,
              let (snapshot, sequence) = pending else { return }
        pending = nil
        let payload: String
        do {
            guard let json = String(data: try JSONEncoder().encode(snapshot), encoding: .utf8) else { return }
            payload = json
        } catch { logger.error("Viewport snapshot is invalid: \(error.localizedDescription, privacy: .public)"); return }
        inFlight = true
        let lifetime = self.lifetime
        // One evaluation is in flight; layout callbacks replace only the latest
        // pending value. The document also rejects any older sequence on replay.
        let script = """
        (() => {
          const value = \(payload), sequence = \(sequence);
          const previous = window.__polarisIosViewport;
          if (previous && previous.sequence > sequence) return;
          window.__polarisIosViewport = {value, sequence};
          const apply = () => {
            const root = document.documentElement, current = window.__polarisIosViewport;
            if (!root || !current) return;
            const value = current.value;
            root.dataset.iosHorizontalSizeClass = value.horizontalSizeClass;
            root.dataset.iosVerticalSizeClass = value.verticalSizeClass;
            root.dataset.iosLayoutSource = 'uikit-viewport';
            root.dataset.iosVerticalBarEdge = value.verticalBarEdge;
            root.dataset.iosVerticalBarSide = value.verticalBarSide;
            root.dataset.iosViewportXPoints = String(value.x);
            root.dataset.iosViewportYPoints = String(value.y);
            root.dataset.iosViewportWidthPoints = String(value.width);
            root.dataset.iosViewportHeightPoints = String(value.height);
            root.dataset.iosKeyboardVisibleBottomPoints = String(value.keyboardVisibleBottom);
            root.style.setProperty('--font-scale', String(value.fontScale));
            // Focus zoom and rotation can change innerWidth before the layout
            // viewport. Convert points against the document's layout width.
            const scale = root.clientWidth > 0 && value.width > 0 ? root.clientWidth / value.width : 1;
            root.style.setProperty('--ios-keyboard-visible-bottom', String(value.keyboardVisibleBottom * scale) + 'px');
            for (const [edge, inset] of [['t',value.top], ['r',value.right], ['b',value.bottom], ['l',value.left]]) {
              root.style.setProperty('--safe-' + edge, String(Math.max(0, inset) * scale) + 'px');
            }
            window.dispatchEvent(new Event('polaris-ios-viewport-change'));
          };
          if (document.documentElement) apply();
          else if (!window.__polarisIosViewportReady) {
            window.__polarisIosViewportReady = true;
            document.addEventListener('DOMContentLoaded', apply, {once: true});
          }
        })();
        """
        webview.evaluateJavaScript(script) { [weak self] _, error in
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                self.inFlight = false
                guard !self.detached else { return }
                if self.lifetime == lifetime {
                    if let error { self.logger.error("Viewport delivery failed: \(error.localizedDescription, privacy: .public)") }
                    else { self.delivered = snapshot }
                }
                self.deliver()
            }
        }
    }

    private static func sizeClass(_ value: UIUserInterfaceSizeClass) -> String {
        switch value { case .compact: return "compact"; case .regular: return "regular"; default: return "unspecified" }
    }
}
