# Frozen iOS Libbox patches

These six files are byte copies of the final unified candidate at
`18202c520f614d3560bf0d4bff27f3e3e0f4a92a` (shared Go
`f63543e77f6af4ca6ef9a756732b43b95d5ddc72`). The construction patch SHA-256 is
`f699001b797fdc473f20ad8738ee674f7244371ed34e383367b3cfa33bad7ec5`.
The superseded `cdbd360f` / `e28b6e99` series is not a build input.
`source-manifest.json` pins every SHA-256, upstream commit, and resulting Git
source tree. Update the shared reviewed candidate before changing these files;
this directory does not maintain an iOS Go fork.

| Patch, in application order | iOS applicability |
| --- | --- |
| transient-command-server | Independent service host; required by strict cleanup patch |
| android-interface-binding | The native dialer branch is Android only; its gomobile platform protocol also adds `BindInterfaceControl` on Apple |
| stopped-speed-strict-cleanup | Strict primary factory, sticky cleanup errors, terminal close, and manager close errors; required by construction patch |
| api-service-close | HTTP API listener shutdown ownership |
| clashmode-hot-switch | Explicit default mode and concurrent mode switching |
| construction-validation | Bound validation result, conservative construction rollback, terminal/detach and save barriers, and six manager replacement publication ordering |

Applying `construction-validation.patch` alone to the pinned upstream commit
fails: it requires the earlier `Box` close-result fields, manager close tests,
and `daemon/started_service_strict_cleanup_test.go` introduced by strict cleanup.
The entire frozen series applies in the recorded order. The source's `go.mod`
pins Go 1.25.5 and SagerNet gomobile v0.1.12; the Android manifest's v0.1.13,
NDK, Java, ELF alignment and ABI settings are not Apple build inputs.

`ios-libbox.py` uses the Apple tags from upstream `buildApple`: shared tags plus
`with_dhcp,grpcnotrace`, and `with_low_memory` for iOS. It binds only
`ios,iossimulator`, with upstream's iOS 15.0 framework deployment minimum.
Polaris itself requires iOS 17.0. Tools, source checkout, and their tests are
isolated under `~/Code`; the supplied core repository provides Git objects.

The build receipt is **build evidence only**. Constructed validation remains
`CleanupUnknown`; the API does not emit `DisposedExact` or `NoOwner`. The
six manager replacement publication-order P2 fix was independently reviewed
and integrated at `fbdb20c082712c24635ada65bb947a7f8032e931`; its final patch
is included here. This does not grant runtime owner or deletion permission.
The sing-tun runtime cleanup P1 remains open. A compiled framework or a
successful close call cannot close that item.

Replay of all six patches on the pinned base produces Git tree
`553461561f628a8875c637d6f248226d8b05fadc`, exactly the shared Go commit's
tree (78 changed paths against the base). Only the reviewed Go patch series
is imported: Apple gomobile, build tags, deployment targets and Swift/Rust
lifecycle guards remain iOS-specific. Receipt verification binds the final
Polaris, shared Go and replacement-review commit IDs as well as patch hashes.

The Mac race gate runs the frozen patch tests, including scope ownership,
rollback, sticky close and real save barriers, with the optional CCM/OCM tags
individually and together. The receipt lists excluded tests by name and reason:
two wildcard HTTP listener fixtures, one synthetic descriptor native setsockopt
fixture, and the Linux fake iptables fixture unsupported by Darwin. Exclusions
are not passed tests or iOS runtime evidence.
