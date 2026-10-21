# RuntimeConfigService source-only handoff

This additive client is authored against the coordinated kernel proto at SHA-256
`b6bb4642d1081f238e5b4f3563b7ddab8e5a1d66b65a306fbd215153164fac5f`.
The vendored file is byte-for-byte identical. RuntimeConfigPlan.no_op is bool field 13. No NOOP enum is invented. Confirmed no-op uses COMMITTED with unchanged identity/control revision. PlanRuntimeConfigRequest.selector_intents is repeated field 6, with RuntimeSelectorIntent group=1, selected=2, expected_selection_path=3, pin_current_tcp=4. The terminal TCP pin fixture explicitly sets pin_current_tcp=true. Mock fixtures verify exact group/selected/path forwarding. The client does not derive or guess physical winners.

## Changes

- `build.rs`: compile both daemon proto files through the existing tonic/prost generator configuration; optional source-contract admission check.
- `proto/runtime_config.proto`: exact authored kernel contract.
- `src/lib.rs`: expose the additive client module and error/size-limit types.
- `src/runtime_config.rs`: six authenticated unary methods on `SingBoxApiClient`, using the existing full-call 2-second deadline and bounded 4 MiB request/response envelopes.
- `runtime_config_wire_check.rs`: optional bounded exact-source comparison.
- `tests/runtime_config.rs`: loopback mock and source-check tests.
- `RUNTIME_CONFIG_HANDOFF.md`: this handoff.

Only `Unimplemented` becomes `RuntimeConfigError::Unsupported`. Other errors remain
`RuntimeConfigError::Call(ClientError)`, retaining transport/status distinctions and
status codes. Capabilities do not imply full-schema hot support. Plan errors,
operation states, lookup state, revisions, incarnation and durability are returned
unchanged. Apply is submitted once without retry: lost response/timeout requires
status reconciliation by the transaction ID, not an assumption of rollback.

The existing mandatory StartedService release guards are unchanged. The released
kernel's expected absence of RuntimeConfigService does not fail those guards.
Setting `POLARIS_RUNTIME_CONFIG_PROTO_SOURCE` explicitly opts into checking that a
provided authored proto matches the vendored contract exactly (64 KiB maximum).
This catches source drift, including types, RPC signatures, enums and fields; it
is deliberately stricter than wire equivalence and is NOT certification of a
compiled kernel's descriptors or service registration. Absence of that environment
variable skips only this optional source check, not the existing release guards.

## Authored validation, not executed

Mock tests cover all six request/response paths and bearer metadata, optimistic
identity and control-revision forwarding, partial capabilities, durability-pending
operations, unknown/expired lookup, Unimplemented for each RPC, authentication and
other status distinctions, real refused transport, bounded timeout with no Apply
retry, bounded explicit persistence-retry timeout without automatic repetition, oversize request rejection, and exact-source mutation/size rejection.

No generators, Cargo builds, native tests, mock tests, CI, publishing or pushes were
run. The Rust and generated binding integration still requires authorized
compilation/testing. The proto source hash was checked by a read-only SHA-256
command; no runtime or hot-apply success is claimed.

The additive AcknowledgeRuntimePersistence RPC forwards exact transaction/active identity, control revision, persistence mode and desired-state digest/revision. Operation fields 13–16 are preserved. Mock fixtures author the new service method and exact receipt fields; no binding generation or compilation has been performed.

Status fields 7–10 preserve persistence mode, durable desired-state digest/revision and owner reload acknowledgement even when no retained operation is returned. Exact mock field forwarding is authored, unexecuted.

The additive RetryRuntimePersistence RPC accepts only the original transaction, active identity, control revision and persistence mode. It never carries a new configuration or desired-state override. One explicit call may rerun a repeat-safe original persistence tail; unknown outcomes require status reconciliation, never automatic retry. The regeneration owner acknowledgement remains a separate RPC. Native repeat-safe callback opt-in and exact generated bindings require their own CI evidence.
