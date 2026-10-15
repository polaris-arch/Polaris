/** Page-independent commit ordering: pending/failed requests never suppress a durable success. */
export function createCommittedChoice() {
  let requested = 0;
  let committed = 0;
  return (write: Promise<void>, apply: () => void): Promise<void> => {
    const revision = ++requested;
    return write.then(() => {
      if (revision > committed) {
        committed = revision;
        apply();
      }
    });
  };
}

// One ordering per global setting, shared across page mounts and navigation.
export const commitThemeChoice = createCommittedChoice();
export const commitLanguageChoice = createCommittedChoice();
