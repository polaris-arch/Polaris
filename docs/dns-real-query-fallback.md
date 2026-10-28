# Default FakeIP and real DNS fallback

## What the setting controls

DNS policy is independent of traffic mode. Switching smart/direct/global does not rewrite a DNS action or choose a different DNS fallback. A traffic rule selecting a proxy is not a DNS rule.

With the default DNS action set to FakeIP, address questions (A and AAAA) select FakeIP. HTTPS (65), SVCB (64), TXT, MX, SRV and other non-address questions cannot receive FakeIP addresses. They now have an explicit `dnsDefaults.realQueryFallbackAction`, configurable in desktop and mobile DNS policy settings:

- `{"type":"server","serverId":"…"}` selects an existing enabled DNS server resource, using that resource's endpoint, bootstrap and outbound settings.
- `{"type":"reject","method":"default"}` rejects the query. The existing `drop` reject method is also accepted.

The same DNS action compiler and server-choice descriptions are reused. Groups, HostsFirst, FakeIP, predefined responses and FollowRouteDefault are not accepted for this field. This deliberately avoids adding response-evaluation/group-failure semantics to the real-query fallback contract.

This fallback applies when the **default action itself is FakeIP**. It does not replace explicit DNS rules, change real-server defaults, or create a second action for every explicit FakeIP rule. A Server action is terminal: upstream errors are returned by that action, not permission to switch to domestic DNS. An unsupported fallback or unavailable/missing server reference compiles to an explicit reject instead of falling through to `dns.final`.

## New installs and upgrades

New installs explicitly select `builtin-remote`. Existing configurations and restored DNS-default objects that lack the field are migrated to explicit `builtin-domestic`, preserving the old real-query destination. An existing explicit value is retained. The missing-field migration is idempotent and does not change the existing schema version. Selective backups with no DNS-default object retain the current defaults; sparse old backups receiving the compatibility field also pass the existing protected-builtin resource migration.

Consequently, upgrading does **not** silently move all existing users' non-address queries to a proxy. Existing users can see their real-query fallback and change it. Raw old config-engine callers without store migration retain the domestic compatibility default; unversioned legacy generation remains unchanged.

`builtin-remote` means the selected server resource, not a guarantee of a particular network exit: users may change its outbound, and the current-exit selector may itself select direct. Check the server's displayed outbound. `DefaultProxyDNS` / `proxyServerId` is a legacy FollowRouteDefault resource reference; it is **not** an all-query-type catchall and does not choose this new fallback.

## Rule order and preserved behavior

The new terminal action follows the existing default FakeIP and real-address shadow rules:

1. Existing LAN/mDNS, managed mesh, probes, node/bootstrap and other protected exceptions retain their relative order.
2. Existing banking and FakeIP-filter exceptions retain their real resolvers and priority.
3. User DNS rules retain their order. A matching explicit Server or Reject action applies before the default fallback, including to non-address queries.
4. Default A/AAAA questions select FakeIP.
5. Internal address lookups that disallow FakeIP still use the existing regional A/AAAA shadow rules and their unconditional address fallback.
6. Only remaining non-address questions reach `realQueryFallbackAction`.

AAAA remains an address question even with IPv6 disabled. The `ipv4_only` strategy and missing IPv6 FakeIP range still control actual response behavior; this change does not claim to enable AAAA responses.

### FakeIP exclusions are not merely “skip FakeIP”

Current ordinary FakeIP exclusions explicitly choose a real resolver before user DNS rules. For example, ordinary NTP/STUN exclusions use domestic DNS; captive-portal exclusions use their existing LAN/system/platform-specific resolver. An overlapping user DNS rule may therefore never be reached. This batch documents that behavior but does not reorder it or silently change those exits. LAN, managed mesh, bootstrap and node-resolution protections are distinct from editable ordinary exclusions.

A future split into “skip FakeIP” and “force resolver” needs a separate compatibility design: persist the old force-resolver choice and priority for existing configurations; present the new semantics explicitly; keep protected LAN/mesh/bootstrap handling separate; and test overlaps with explicit DNS rules and every record type. It is not introduced here.

## Verification and limits

The regression suite covers three traffic modes, IPv6 on/off, A/AAAA/HTTPS/SVCB/TXT/MX/SRV/CAA/NS, domestic/remote/reject/missing selections, internal address lookups, explicit DNS precedence, protected exceptions, migration and backup compatibility. A reviewed JSON snapshot locks the default rule tail; frozen legacy config snapshots are intentionally unchanged. DHCP fallback participates in the existing on-demand resource dependency traversal.

These checks concern queries that enter the core DNS rule chain. They do not establish interception of browser DoH, arbitrary application resolvers, OS DNS, bootstrap bypass sockets or platform interfaces. Packet-capture/native-platform privacy guarantees require separate verification. No Windows token, ACL, service, launcher, kernel takeover or kill-switch behavior changes in this batch.
