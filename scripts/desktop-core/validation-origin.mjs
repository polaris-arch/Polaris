// Narrow technical-validation admission: only the committed strict release can
// admit native payload proof for its exact App checkout and Actions attempt.
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { formalGateContext } from './native-dns-gates.mjs';
import { requireGraph } from './source-graph.mjs';

export function validationContext(candidate, env = process.env, root = process.cwd(), run) {
  // The optional Git reader is a unit-test seam; the CLI never accepts an alternate reader.
  const context = formalGateContext(root, env, run);
  requireGraph(context.candidate === candidate, 'Formal native proof App candidate differs');
  return { runId: context.runId, attempt: context.attempt };
}

// Package's Bash setup and the Python native/final proof use the same admission
// function. The contract is read from actual source bytes.
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  requireGraph(process.argv.length === 3, 'Validation context requires exactly one candidate');
  console.log(JSON.stringify(validationContext(process.argv[2])));
}
