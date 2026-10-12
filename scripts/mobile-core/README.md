# Formal mobile source and binding input

This is the Polaris App producer input, separate from the completed box CLI and
OpenWrt producers. The source candidate is the actual common immutable
`577d4b9a089b71d2f0a4ca334b87264b55c10478`, tree
`072dacb999c07c7948816f10efaaaf371da45eb9`, annotated tag
`v1.15.0-alpha.12` / object `5cedbe7a54594756da544af90ebfd1f403f2e091`.
The tag's CLI/OpenWrt publication does not approve a mobile output. No existing
rule requires a second M tag. The mobile role is explicit in this input and its
future producer receipt; it is not retroactively added to the raw tag annotation.

The formal tool choice is Go **1.25.5**, gomobile and gobind **v0.1.13** for both
mobile platforms. That matches the actual source go.mod, Makefile and generated
binding API. The historical v0.1.12 Apple policy/receipts retain their identity.
Diagnostic v0.1.13 results also remain diagnostic; they are not formal output
pins. No global Go/gomobile/NDK installation or upgrade is performed here.

The source input uses no App overlay or historical patch replay. Verification
checks the actual tag object, peel, commit, tree, go.mod/go.sum, source-owned tag
declaration and clean compiler-input inventory. `fork_input.py` does not require
the old nonempty patch list or a CLI source receipt.

Formal binding plans are explicit:

- Android: one API24 AAR, `arm64-v8a, armeabi-v7a, x86, x86_64`, 16KiB PT_LOAD
  requirement, Java17 and pinned r30-beta3 NDK. The producer uses a private SDK
  view containing only that NDK; ambient installed NDK28 cannot win a fallback
  lookup. It does not invoke the helper that also builds API21/no-Naive legacy.
- Apple: only `ios/arm64, iossimulator/arm64, iossimulator/amd64`, static
  XCFramework, library deployment minimum15 and App minimum17. No default
  macOS/tvOS targets. Actual selected Xcode/SDK/compiler identities must be
  collected before execution and checked by the eventual producer.
- Tags are copied from the actual source-owned libbox declaration, including
  gvisor, Naive, Tailscale and Apple-only DHCP/low-memory flags. The Go module
  remains `github.com/sagernet/sing-box`; only outer assets use `polaris-box-*`.
- Version is passed through fixed ldflags. `ReadTag`/`git describe`, an upstream
  cache tag, or a reused CLI BuildID cannot provide formal mobile identity.

`plan.py` verifies the input and prints a bind plan. It never executes a build,
copies an SDK into an App, installs tools, creates a tag or publishes assets.
Plan fingerprints bind source and target declarations; actual tool identities
must be added by the producer before compiling. `android_sdk.py` prepares the
scoped SDK view only under a fixed private work slot held by the caller's
exclusive producer lease. Tests are authored in `test_inputs.py` and unexecuted.

The explicit planning entry is `bash scripts/plan-mobile-core.sh android` or
`apple`, followed by `--source`, `--gomobile`, `--output`, and Android's
`--bootclasspath`. Supply the existing exact source checkout and isolated tool
paths. Planning does not resolve or download an SDK. Existing legacy build entry
bytes are kept intact so old frozen receipt/script identities remain resolvable.

The formal producer runner/actual output verifier/cache consumer wiring still
has to consume this input. Required actual results are four-ABI ELF/API/JNI and
module/BuildID checks, three static Apple architecture slices and generated API,
and final APK/R8/alignment/App+PacketTunnel static-link provenance. Old
`scripts/libbox-patches` and Apple six-patch inputs remain historical inputs;
their receipts cannot be relabeled. The input has no artifact pins, producer
receipt, App/device acceptance or publication eligibility. A plan, CLI output
or diagnostic selfcheck cannot fill those fields.
