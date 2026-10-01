/** A report refresh publishes only the latest request, including its failure state. */
export interface ReportLoadState<T> {
  report: T | null;
  loading: boolean;
  error: boolean;
}

export function createLatestReportLoader<T>(
  load: () => Promise<T>,
  publish: (state: ReportLoadState<T>) => void,
) {
  let generation = 0;
  return {
    refresh() {
      const mine = ++generation;
      // An older result is no longer a result for the current saved config.
      publish({ report: null, loading: true, error: false });
      void load().then(
        (report) => {
          if (mine === generation) publish({ report, loading: false, error: false });
        },
        () => {
          if (mine === generation) publish({ report: null, loading: false, error: true });
        },
      );
    },
    invalidate() {
      generation += 1;
    },
  };
}
