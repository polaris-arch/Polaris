import { draftFromSpecs, type FieldSpec, type FormValues } from '@/components/dialogs/field-spec';
import { allFields, nodeFormGroups, type NodeFieldGroupId, type NodeProto } from '@/components/dialogs/node-spec';

/** Display a saved optional group only when its currently visible values differ from this protocol's real blank draft. */
function configured(field: FieldSpec, draft: FormValues, defaults: FormValues): boolean {
  if (field.when && !field.when(draft)) return false;
  const value = draft[field.k];
  const fallback = defaults[field.k];
  if (field.t === 'switch') return value === true && value !== fallback;
  if (field.t === 'number') return typeof value === 'number' && Number.isFinite(value) && value !== fallback;
  if (field.t === 'select') return (value ?? '') !== (fallback ?? '');
  return (typeof value === 'string' ? value.trim() : '') !==
    (typeof fallback === 'string' ? fallback.trim() : '');
}

/** Initial presentation only. Manual folds remain under the user's control after mount. */
export function initialNodeFormGroups(
  proto: NodeProto,
  draft: FormValues,
  options?: {
    editing?: boolean;
    detour?: string;
    bindInterface?: string;
    bindInterfaceEditable?: boolean;
  },
): ReadonlySet<NodeFieldGroupId> {
  const groups = nodeFormGroups(proto);
  const open = new Set<NodeFieldGroupId>(['basic']);
  if (groups.some(group => group.id === 'transport')) open.add('transport');
  if (options?.editing) {
    const defaults = draftFromSpecs(allFields(proto));
    for (const group of groups) {
      if (group.id === 'routing' || group.id === 'advanced') {
        if (group.fields.some(field => configured(field, draft, defaults))) open.add(group.id);
      }
    }
  }
  // These two shared controls live in the mobile advanced group, outside ND_SPEC.
  if (options?.detour || (options?.bindInterfaceEditable && options.bindInterface)) open.add('advanced');
  return open;
}

export function expandedNodeFormGroups(
  previous: ReadonlySet<NodeFieldGroupId>,
  group: NodeFieldGroupId,
): ReadonlySet<NodeFieldGroupId> {
  return new Set([...previous, group]);
}
