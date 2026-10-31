# Linux development selection

The default Cargo feature is `formal-selection`. An explicitly separate
`local-development-selection` compiler lane supports Linux amd64's existing
sealed-FD provider. Both or neither selection features are compile errors.
macOS and Windows local execution remain unsupported.

A developer compilation is the trust decision. Source hashes and local probe
records are provenance, not signatures. No runtime environment variable,
external receipt, JSON file, or validation-only carrier can change the compiled
lane, member pins, or purposes.

`scripts/desktop-core/local-development-source.json` is the reviewed source,
toolchain, target, feature and sidecar recipe. The Core source and the producer
source are separately pinned clean Git trees. The producer builds the real Core
from that source, captures actual buildinfo, creates its recovery archive, and
runs the production protected capability/configuration and Cronet FD probes.
It then generates `selected_core/development_config.rs` with exact compiler
pins. Fixed-slot data in `resources/.local-development/` must match those pins;
it is not independently authoritative.

After provisioning the normal locked dependency cache, under the existing
native build lock and resource budget:

1. Run `node scripts/desktop-core/local-development.mjs collect` with explicit
   `--core-source`, `--producer-source`, fresh `--core` build-output path and
   pinned `--sidecar` input. No GitHub workflow identity is accepted or invented.
2. Build the Helper with `--no-default-features --features
   local-development-selection`, using the normal Helper target/toolchain.
   Copy its final executable to
   `resources/.local-development/linux/polaris-helper`.
3. Run `node scripts/prepare-core-identity.mjs --platform=linux
   --selection=local-development
   --directory=resources/.local-development/linux --mode=prepare`.
   This verifies the actual final members and freezes the actual Helper digest.
4. Compile the App with `--no-default-features --features
   local-development-selection`. Its installed selection must be byte-identical
   to the execution crate's compiled selection.

The Helper compiles only the pure Core selection. Its own future digest is
excluded, so there is no Helper self-reference. The App separately freezes the
final installation identity. Changing the source recipe or selection requires
regeneration and compilation; changing runtime data cannot select another Core.

Actual operation admission remains with the existing nonserializable provider,
protected cohort and fresh operation lease. Warm metadata lookup does not hash
the payload again. The recovery archive uses the existing exact member and
inventory validator under the selected resource root.

Formal preparation, packaging and publication reject the local input schema,
input slot and compiled local executable marker. The marker is a rejection aid,
not authentication. This lane does not create a release policy, signed receipt,
installer privilege, or installed Helper. System installation and live network
changes still require their normal explicit boundary.
