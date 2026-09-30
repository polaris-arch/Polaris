#!/usr/bin/env node

// Publishing is frozen until PC owner stop/custody and platform acceptance
// close the release blockers. Candidate builds remain available for validation.
// No version, environment flag, argument, or helper ACK grants clearance.
console.error(
  'PC_RUNTIME_RELEASE_BLOCKED: sing-tun PC runtime Start/PostStart, internal rollback, stale cleanup, '
  + 'owner stop/custody, and macOS/Windows native completion have not completed runtime and platform '
  + 'acceptance. Desktop artifacts are controlled validation '
  + 'candidates; draft creation, release uploads, and public promotion are blocked.',
);
process.exitCode = 1;
