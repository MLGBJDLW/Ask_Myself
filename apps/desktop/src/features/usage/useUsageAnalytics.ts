import { useCallback, useEffect, useState } from 'react';

import * as api from '../../lib/api';

export function useUsageAnalytics(initialFilter: api.UsageAnalyticsFilter = {}) {
  const [filter, setFilter] = useState<api.UsageAnalyticsFilter>(initialFilter);
  const [data, setData] = useState<api.UsageAnalytics | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [revision, setRevision] = useState(0);

  const reload = useCallback(() => setRevision((value) => value + 1), []);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    api.getAiUsageAnalytics(filter)
      .then((analytics) => { if (!cancelled) setData(analytics); })
      .catch((reason: unknown) => { if (!cancelled) setError(String(reason)); })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [filter, revision]);

  return { filter, setFilter, data, loading, error, reload };
}
