# Actual App simulator acceptance

This separate XCUITest project opens the already installed `com.polaris.app`.
It checks five navigation destinations, reachable controls inside the window,
portrait/landscape, termination followed by relaunch, and rotation while an unsaved
editing draft is open. The default gate keeps the keyboard visible while checking
Name and Save above its accessory. The separate maximum accessibility text case
may use the actual system Done control before checking Save and retained draft.
Screenshots and the accessibility hierarchy are kept in the result bundle.
The editing gate also rejects overlap between the real sheet title and the
focused field, and requires its full frame inside the actual App window and
native WebView intersection. Name and the import textarea both use the production
`m-form-input` inner focus outline (`2px`, offset `-2px`); a fixed extra 2pt
outside margin would misclassify an intact inner border. Screenshots establish
whether the border is actually visible, alongside the frame/heading gates.
Accessibility hittability alone does not establish visual visibility.
Rotation now waits for the native window, WebView, field, footer and keyboard
frames to be equal on two consecutive observations. Each pose explicitly
requires a nonzero software keyboard intersecting the App window, with up to 10s
for that actual state before geometry settling; an AX keyboard with zero height
cannot satisfy it. Stages after actual Done/Hide do not require keyboard presence.
All positive `SystemInputAssistantView` and `inputView` matches, plus actual Done
toolbars, are intersected with the App window and included in geometry settling.
The earliest top among all observed frames is the conservative visible boundary;
duplicate `inputView` elements do not resolve to a random first match.
The maximum text case also uses the actual dismissal control when Save reports
as hittable but lies below the keyboard/accessory or outside the window.

The optional `testTsConfigurationSaveWithoutLogin` mutates simulator App data;
exclude it from layout-only acceptance. No draft-save or VPN action is needed.

After building and installing the App, run on the Mac:

```sh
cd scripts/ios-simulator-acceptance
xcodegen generate
xcodebuild test -project PolarisSimulatorAcceptance.xcodeproj \
  -scheme PolarisSimulatorAcceptance -destination 'id=<SIMULATOR_UDID>' \
  -derivedDataPath DerivedData -resultBundlePath acceptance.xcresult \
  CODE_SIGNING_ALLOWED=NO \
  -only-testing:PolarisUITests/PolarisUITests/testAllDestinationsAndLandscape \
  -only-testing:PolarisUITests/PolarisUITests/testRelaunchRetainsUsableNavigation \
  -only-testing:PolarisUITests/PolarisUITests/testRotationKeepsEditingDraft
```

Use a conventional iPhone and an iPad destination. Run
`testAccessibilityRotationKeepsEditingDraft` separately after setting that test
simulator's content size to `accessibility-extra-extra-extra-large`; restore
`large` afterwards. Current Xcode 27.1 has the Duo device type but the installed
27.0 runtime cannot boot it; no 27.1 runtime or Duo hardware is available.
Duo outer/inner screens and half-fold reserved-region avoidance remain
unverified. Approximate viewport checks do not count as Duo acceptance. Result bundle paths must be new for each
run. The tests do not create VPN profiles or change system routes. They provide
UI/runtime launch evidence, not tunnel traffic, cleanup or NoOwner evidence.

## Dedicated iPad OS window resize

Run these tests separately on the dedicated conventional iPad at default `large`
text size. The operator owns the simulator and must record/restore its OS window
mode. `testInspectIPadMultitaskingSettings` opens actual Settings, captures its
complete accessibility hierarchy, selects the actual
`com.apple.settings.multitaskingAndGestures.windowedApps` button when needed,
and requires it to report selected. Its result only describes setup; it is not
resize acceptance. A missing control or selection is a preserved setup failure.

`testInspectIPadSettingsWindowMenu` separately discovers Settings' native window
menu. It launches actual Settings, captures the initial App/SpringBoard AX and
images, and taps the exact Settings status-bar name 设置 observed in R16. It
requires a SpringBoard menu window containing Settings' App-name button, opens
its enabled, hittable 窗口 button, and captures the popup. It then opens the
enabled, hittable 移动与调整大小 button and captures the real submenu. Initial and
final Settings AX PID must match and Settings must remain foreground. No layout
button, window control container or unknown submenu action is clicked; Polaris
is not activated and no draft, node or VPN is saved. This is native Settings menu
discovery only, not a repair of OS behaviour or paired-window acceptance. All
existing mode-setup actions, shared helpers and strict cases remain unchanged.
Run it alone using a new result bundle and
`-only-testing:PolarisUITests/PolarisUITests/testInspectIPadSettingsWindowMenu`.

Apple documents enabling Windowed Apps or Stage Manager in Multitasking &
Gestures and resizing with the bottom-right OS handle. See [Work with multiple
windows on iPad](https://support.apple.com/en-am/guide/ipad/ipad08c9970c/ipados).
This test exercises that windowing mechanism, not legacy Split View, paired-app
tiling, or a Duo posture. No browser viewport or system defaults are injected.

Use new result bundles and a separate harness derived-data directory:

```sh
# Optional one-time setup inspection; launches Settings and may change OS window mode.
xcodebuild test -project PolarisSimulatorAcceptance.xcodeproj \
  -scheme PolarisSimulatorAcceptance -destination 'id=<DEDICATED_IPAD_UDID>' \
  -derivedDataPath <NEW_HARNESS_DD> -resultBundlePath <NEW_SETUP_RESULT>.xcresult \
  CODE_SIGNING_ALLOWED=NO \
  -only-testing:PolarisUITests/PolarisUITests/testInspectIPadMultitaskingSettings

xcodebuild test -project PolarisSimulatorAcceptance.xcodeproj \
  -scheme PolarisSimulatorAcceptance -destination 'id=<DEDICATED_IPAD_UDID>' \
  -derivedDataPath <NEW_HARNESS_DD> -resultBundlePath <NEW_RESIZE_RESULT>.xcresult \
  CODE_SIGNING_ALLOWED=NO \
  -only-testing:PolarisUITests/PolarisUITests/testIPadWindowResizeKeepsEditingDraft
```

The resize case starts in a settled native portrait window, opens an unsaved
Tailscale draft, then narrows and expands the real OS window. It dismisses the
docked keyboard using its actual control to reach the OS handle and continues
typing a short synthetic suffix into the still-focused Name after each drag,
without tapping or changing the selection. The initial inserted token must occur
exactly once in the actual field value. Expected values insert each suffix after
that known token, maintaining the real insertion point even if it precedes the
default Tailscale text; full-value equality is required after every continuation.
Both the App window and native WebView widths
must change in the intended direction by more than 80 points, including after
the keyboard/layout settles. Movement, rotation or keyboard-only viewport
changes cannot satisfy this width gate. Drag coordinates, initial/before/final
native frames, App AX PID, draft continuity, screenshot and full AX are retained.
The App must remain foreground during resize and final settling. AppSwitcher
previews scale both window and WebView AX frames too; their dimensions alone
cannot establish resize. The test also requires a real keyboard after continued typing,
stable final widths and images showing the actual foreground form. Coordinates
are relative normalized points in the native window element, without subtracting
physical `screenPoint` from differently rotated AX frames. This first window
case uses portrait; landscape window resize remains a separate acceptance item.

At each size, the software keyboard must exist, have nonzero height and intersect
the App window. Name and Save must remain inside the App/WebView bounds and above
the earliest actual keyboard/input assistant/accessory frame. The sheet title
must not overlap Name. The default window case does not dismiss the keyboard to
make Save pass. It does not call the fullscreen navigation assertion, whose iPad
rail expectation is unsuitable for compact window traits.

All screenshots/AX are captured before geometry assertions and kept on failure.
Export and visually review Name's complete focus border and Save above the
accessory; accessibility frames alone are insufficient. If the OS handle fails
to resize either native frame, preserve that failure rather than report split
screen acceptance. Run default rotation and the separate maximum-text rotation
case with the same fresh harness as additional keyboard checks; do not combine
old harness results with this new gate. No draft is saved and no VPN is started.

## Multiline and system-control inspection

`testRotationKeepsMultilineDraft` opens Nodes → Add → Import nodes and enters nine
synthetic plain-text lines in the existing rows=7 import textarea. It rotates
through landscape and portrait, keeps the original value/PID, then appends a short
synthetic suffix at the original insertion point in each settled pose. It does
not tap the textarea, select all or replace the draft between poses. The expected
value accumulates both suffixes. It requires keyboard
focus, the complete textarea frame/border above the actual input assistant and
keyboard, and no overlap with the real heading. Captures after continued typing
must be reviewed for the caret and last edit being revealed: frame containment
alone does not prove the textarea's internal scroll position. After both poses,
it uses the real Done/Hide Keyboard control once and requires the untouched Parse
button inside the visible native window. It never taps Parse, Import, Save, or
the file picker.

Run it with a new result bundle and the fresh harness:

```sh
xcodebuild test -project PolarisSimulatorAcceptance.xcodeproj \
  -scheme PolarisSimulatorAcceptance -destination 'id=<SIMULATOR_UDID>' \
  -derivedDataPath <NEW_HARNESS_DD> -resultBundlePath <NEW_MULTILINE_RESULT>.xcresult \
  CODE_SIGNING_ALLOWED=NO \
  -only-testing:PolarisUITests/PolarisUITests/testRotationKeepsMultilineDraft
```

`testInspectIPadSystemWindowControls` requires an actual OS window exposing the
observed SpringBoard button `window-controls:com.polaris.app`. It captures the
foreground App and SpringBoard, then touches and holds that actual button for
one second and captures the resulting menu/full AX. Apple's [windowing guide](https://support.apple.com/guide/ipad/work-with-multiple-windows-at-the-same-time-ipad08c9970c/ipados)
describes holding window controls to reveal arrangement options. The inspection
does not guess localized tiling labels or click unknown menu actions. Use its
result to plan actual two-App side-by-side tiling;
free window resizing alone does not satisfy that separate requirement or prove
legacy Split View. All failed R1/R2 artifacts remain historical evidence; new
harness results and source SHA must be recorded separately.

`testInspectIPadKeyboardControls` is a separate discovery run for the actual iPad
hidden-keyboard state. It opens an unsaved draft, focuses Name and types only the
synthetic suffix ` Keyboard recovery inspection`, then captures the real state.
Typed value is not keyboard visibility proof: only a nonzero keyboard rectangle
intersecting the native window counts as visible. When that is absent, it
taps the observed `inputAssistantView` button labelled Keyboard/键盘/鍵盤.
The resulting menu screenshot/full AX must establish the actual Show Keyboard
control before a recovery helper is implemented. It does not click guessed menu
labels, change simulator keyboard preferences, or count discovery as keyboard
acceptance. An already visible software keyboard is captured without menu taps.

## Full-window preparation and two-App tiling

R6 uses the actual menu buttons observed in the dedicated iPad's R5 SpringBoard
AX. It locates their existing image identifiers: Fill/填充 is
`rectangle.inset.filled`, and the side-by-side preset/左右 is
`inset.filled.lefthalf.righthalf.rectangle`. No unseen localized menu label is
guessed. Missing window controls or menu actions fail after retained captures.

`testPrepareIPadFullWindow` is an isolated, repeatable setup run. When both the
initial native App window and WebView already occupy the actual SpringBoard
display width, it only captures that state and requires the same PID and two
adjacent valid foreground frames at full width. It does not click Fill or claim
the window widened. From a retained narrow OS window, it holds the real Polaris
controls and chooses Fill, then still requires both native widths to increase by
more than 80pt, settle and occupy the display width. Either branch is only window
preparation, not keyboard, resize or two-App acceptance.

`testIPadPairedWindowsKeepsEditingDraft` opens the same unsaved Name draft and
establishes the strict real-keyboard gate, then launches actual Preferences as
the second App. It returns with `app.activate()`, preserving the original Polaris
PID/value; it never calls `app.launch()` during this draft. From the real Polaris
window-controls menu it chooses the observed side-by-side preset. Both actual
App windows must settle without overlap, occupy less than 80% of display width,
align at their top/height and retain at least 65% of display height. Polaris and
its native WebView must change width by more than 80pt, including in the final
keyboard state. Preferences must have real reachable text inside its window.
The focused field height must not uniformly shrink like an AppSwitcher preview;
retained images must independently confirm normal font sizes and actual reflow.

The layout helper first holds the observed collapsed
`window-controls:com.polaris.app` button. If that button is missing, it uses the
actually inspected native Window → Move & Resize route: while Polaris is
foreground, reveal the menu through the exact Polaris status-bar name if needed,
scope to the SpringBoard window containing its App-name button, tap the enabled,
hittable exact 窗口 button, then the observed 移动与调整大小 submenu entry. It
never blindly presses the expanded 窗口控制 Other container or its Close,
Minimize and Unzoom children. Both menu routes then require the specified
observed image identifier's parent button to exist, be enabled and be hittable.
The inspected Move & Resize submenu contains Left and side-by-side identifiers;
Fill was only observed in the collapsed-controls menu. If a requested Fill is
absent, the common wait fails and keeps its screenshots; no guessed replacement
action is used. Apple's [menu-bar guide](https://support.apple.com/guide/ipad/use-the-menu-bar-ipadb4ede9db/ipados)
describes the actual App-name entry. No unseen
English label or unrelated App's controls are guessed. It fails if the OS does
not pair Preferences, if a stale AX window
suggests two windows without visible content, or if the software keyboard is
missing/zero-height. Default Name/Save/heading/input-assistant containment gates
remain strict. The operator prepares and records the actual host keyboard state;
the harness changes no keyboard preference, injects no viewport, and does not
substitute keyboard absence for a pass. SpringBoard/Preferences/App screenshots
and full AX are kept before assertions. The real divider is captured for later
discovery; this revision does not drag an unknown divider or claim its resizing
has passed. No node is saved, no parser/import is called, and no VPN starts.

Run each setup/acceptance in its own fresh result bundle with the dedicated iPad:

```sh
xcodebuild test -project PolarisSimulatorAcceptance.xcodeproj \
  -scheme PolarisSimulatorAcceptance -destination 'id=<DEDICATED_IPAD_UDID>' \
  -derivedDataPath <NEW_HARNESS_DD> -resultBundlePath <NEW_FULL_WINDOW_RESULT>.xcresult \
  CODE_SIGNING_ALLOWED=NO \
  -only-testing:PolarisUITests/PolarisUITests/testPrepareIPadFullWindow

xcodebuild test -project PolarisSimulatorAcceptance.xcodeproj \
  -scheme PolarisSimulatorAcceptance -destination 'id=<DEDICATED_IPAD_UDID>' \
  -derivedDataPath <NEW_HARNESS_DD> -resultBundlePath <NEW_PAIRED_RESULT>.xcresult \
  CODE_SIGNING_ALLOWED=NO \
  -only-testing:PolarisUITests/PolarisUITests/testIPadPairedWindowsKeepsEditingDraft
```

Two-App tiling is the current Windowed Apps mechanism, not a claim of legacy
Split View. Its actual runtime and images must pass separately from free-window
corner resizing, rotation-only cases, or setup inspection. The operator restores
the dedicated simulator's OS layout/settings after evidence is retained.

## Window presets during continuous editing

`testIPadWindowPresetsKeepsEditingDraft` is an independent ordinary-iPad case
starting from an operator-prepared actual full window. It uses the observed
top-bar Left action (`inset.filled.lefthalf.rectangle`) and then Fill
(`rectangle.inset.filled`) to change native Window/WebView width by more than
80pt in each direction. It keeps one Polaris PID, field and unsaved synthetic
draft; no keyboard Hide, field tap, App activation or relaunch occurs between
presets. The strict nonzero keyboard/Name/Save/heading/accessory geometry gate
must pass after each actual resize before continued input. If the OS hides the
keyboard or stops real editing, that is a retained failure rather than a bypass.

At each size it appends a short suffix at the actual known initial-token insertion
point, requires exact complete-field equality, then repeats the real keyboard
and geometry gates and final native width checks. The Name frame must retain its
initial height within 1pt pixel alignment rather than uniformly scale like a
preview. Original images must show the caret/continued input, normal font reflow,
and complete Name/Save above all observed input-assistant frames. No Save or VPN
action is taken. This is a single-App OS window test, not two-App tiling or divider
acceptance. The separate corner-resize case and all its failed artifacts remain
unchanged; a preset result does not retroactively pass the corner path.

After actual full-window preparation, use a new result bundle:

```sh
xcodebuild test -project PolarisSimulatorAcceptance.xcodeproj \
  -scheme PolarisSimulatorAcceptance -destination 'id=<DEDICATED_IPAD_UDID>' \
  -derivedDataPath <NEW_HARNESS_DD> -resultBundlePath <NEW_PRESET_RESULT>.xcresult \
  CODE_SIGNING_ALLOWED=NO \
  -only-testing:PolarisUITests/PolarisUITests/testIPadWindowPresetsKeepsEditingDraft
```

R9 setup erratum: earlier revisions assumed the full-window menu was already
revealed and required a narrow-to-full transition even when preparation was
already complete. The R8 setup failure and its screenshots/AX remain unchanged.
The new already-full setup result cannot be counted as window resizing. No
corner, preset, paired-window, continued-input or actual-keyboard gate is reduced
by this setup/menu correction; each still needs its own fresh run and images.

## Native Window menu discovery

`testInspectIPadWindowMenu` is a separate discovery case. It opens an unsaved
synthetic Name draft and first requires the actual positive keyboard plus strict
Name/Save/accessory geometry. It reveals the menu using only the observed exact
Polaris status-bar name when needed, scopes to the SpringBoard menu window
containing the Polaris App-name button, then taps its actual button labelled 窗口.
It captures the popup screenshot and complete SpringBoard/App AX, then taps the
observed enabled, hittable button labelled 移动与调整大小 in that Polaris menu
window and captures the actual submenu and App. The PID and full draft value
must remain unchanged. It does not choose an unseen submenu layout action.
Initial real keyboard evidence and menu discovery do not prove keyboard
continuity after either menu click. Discovery is not resize or paired-window acceptance, and the
R9 failure showing 窗口控制 as an Other instead of a Button remains unchanged.

Run only this discovery on the dedicated iPad with a new result bundle:

```sh
xcodebuild test -project PolarisSimulatorAcceptance.xcodeproj \
  -scheme PolarisSimulatorAcceptance -destination 'id=<DEDICATED_IPAD_UDID>' \
  -derivedDataPath <NEW_HARNESS_DD> -resultBundlePath <NEW_WINDOW_MENU_RESULT>.xcresult \
  CODE_SIGNING_ALLOWED=NO \
  -only-testing:PolarisUITests/PolarisUITests/testInspectIPadWindowMenu
```

## Dock discovery during editing

`testInspectIPadDockWhileEditing` is an independent inspection starting in a
prepared actual full-width portrait Polaris window. It opens an unsaved synthetic
Name draft and first requires the positive software keyboard and strict
Name/Save/heading/accessory geometry. Using the actual positive SpringBoard
`SBSwitcherWindow:Main` bounds, it drags from the bottom centre (2pt inside the
screen) up to 120pt above the bottom. Normalized local coordinates delegate the
screen conversion to XCTest; the actual frame, origin and gesture screen points
are retained alongside complete SpringBoard/App AX and original screenshots.

The final Polaris PID, whole draft value and running-foreground state must remain
unchanged; Home/AppSwitcher is a failure. The snapshots discover the actual Dock
and available App icons. No Preferences launch, App-icon tap or drag, unseen Dock
label, Save or VPN action is performed. This inspection does not assert keyboard
continuity after the system gesture and does not pass two-App tiling. Apple's
[window-layout guide](https://support.apple.com/en-nz/guide/ipad/ipadfe7c65e9/ipados)
describes the Dock entry for arranging windows; a future action still needs the
observed icon and fresh strict geometry evidence. All R12 helper and strict cases
remain unchanged; the earlier paired-window failure stays recorded.

After actual full-window preparation, run only this inspection with a new result
bundle:

```sh
xcodebuild test -project PolarisSimulatorAcceptance.xcodeproj \
  -scheme PolarisSimulatorAcceptance -destination 'id=<DEDICATED_IPAD_UDID>' \
  -derivedDataPath <NEW_HARNESS_DD> -resultBundlePath <NEW_DOCK_RESULT>.xcresult \
  CODE_SIGNING_ALLOWED=NO \
  -only-testing:PolarisUITests/PolarisUITests/testInspectIPadDockWhileEditing
```

## Two App windows from the observed Dock icon

`testIPadDockPairedWindowsKeepsEditingDraft` is independent of the menu-preset
paired case. Start with an actual full-width portrait Polaris window. After the
initial strict positive-keyboard/Name/Save gate, it repeats the observed short
Dock reveal and requires the enabled, hittable Icon 设置 inside `Multitasking Dock`
observed in R13's actual AX. It drags that icon's centre to 20pt inside the
screen's right edge at mid-height, pressing for 1s, moving slowly and holding
for 1s. Actual screen/icon/gesture coordinates, App/SpringBoard/Settings AX and
original images are kept. The public
[XCTest drag API](https://developer.apple.com/documentation/xcuiautomation/xcuicoordinate/press%28forduration%3Athendragto%3Awithvelocity%3Athenholdforduration%3A%29)
supports explicit velocity and the final hold.

R15 changes only the initial press from 0.5s to 1s as one controlled automation
experiment. R14's actual failure and video showed no Settings icon lift; its
coordinates, velocity, final hold and all acceptance gates stay unchanged. Apple
does not guarantee that this duration establishes an icon drag. If no lift is
observed again, the automation gesture prerequisite remains unestablished; that
does not identify a product defect or justify further parameter cycling.

No Settings launch, App activation/relaunch, layout-preset action or unknown
icon is used. The case requires two stable nonempty, nonoverlapping native App
windows, changed Polaris Window/WebView widths, full screen containment, matched
window height/top, reachable Settings content and an unscaled Name height. An
unsuccessful OS drop remains a captured failure. Normal font/reflow and actual
two-App visibility still require review of the original images.

After the drop it explicitly taps the same Name field to restore editing, then
requires the unchanged PID and full draft plus a positive real keyboard and
strict Name/Save/heading/accessory bounds. It types one distinct suffix; removing
that suffix exactly once must reproduce the entire original draft. It repeats
the strict keyboard/field/footer gate and keeps continued-input screenshots.
This proves editing after explicit focus restoration, not preservation of the
original caret across the OS drag. The separate preset case still requires its
original focus/continued input without a field tap. No Save or VPN action occurs.
The original paired, preset and corner cases and their earlier failures are
unchanged; R13 Dock inspection does not pass this new two-App case. The final
keyboard gate settles Polaris; Settings has a final frame/content check and
images rather than an automatic proof of all subsequent second-window motion.

Use a new result bundle after actual full-window preparation:

```sh
xcodebuild test -project PolarisSimulatorAcceptance.xcodeproj \
  -scheme PolarisSimulatorAcceptance -destination 'id=<DEDICATED_IPAD_UDID>' \
  -derivedDataPath <NEW_HARNESS_DD> -resultBundlePath <NEW_DOCK_PAIRED_RESULT>.xcresult \
  CODE_SIGNING_ALLOWED=NO \
  -only-testing:PolarisUITests/PolarisUITests/testIPadDockPairedWindowsKeepsEditingDraft
```

## Foreground Settings system-window discovery

The multitasking settings inspection also preserves the actual SpringBoard
image and full accessibility hierarchy immediately after Settings launches.
This only discovers the second App's native window/menu controls. It changes
no layout or mode action and proves no two-App tiling. The failed Dock drag
experiment is retained; no further drag-parameter adjustment follows it.
