import { useState, useEffect, useCallback } from "react";
import * as api from "../lib/api";
import type { Source, IngestResult } from "../types";
import { LoadingSpinner } from "../components/LoadingSpinner";

export function SourcesPage() {
  const [sources, setSources] = useState<Source[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [scanResults, setScanResults] = useState<Record<string, IngestResult>>({});
  const [scanningId, setScanningId] = useState<string | null>(null);
  const [scanningAll, setScanningAll] = useState(false);

  // ── Add source form ───────────────────────────────────────────────────
  const [showAddForm, setShowAddForm] = useState(false);
  const [formPath, setFormPath] = useState("");
  const [formInclude, setFormInclude] = useState("**/*.md");
  const [formExclude, setFormExclude] = useState("");
  const [adding, setAdding] = useState(false);

  const loadSources = useCallback(async () => {
    try {
      const list = await api.listSources();
      setSources(list);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    loadSources();
  }, [loadSources]);

  const handleAdd = async () => {
    if (!formPath.trim()) return;
    setAdding(true);
    setError(null);
    try {
      const includeGlobs = formInclude
        .split(",")
        .map((s) => s.trim())
        .filter(Boolean);
      const excludeGlobs = formExclude
        .split(",")
        .map((s) => s.trim())
        .filter(Boolean);
      await api.addSource(formPath.trim(), includeGlobs, excludeGlobs);
      setFormPath("");
      setFormInclude("**/*.md");
      setFormExclude("");
      setShowAddForm(false);
      await loadSources();
    } catch (e) {
      setError(String(e));
    } finally {
      setAdding(false);
    }
  };

  const handleDelete = async (sourceId: string) => {
    setError(null);
    try {
      await api.deleteSource(sourceId);
      await loadSources();
    } catch (e) {
      setError(String(e));
    }
  };

  const handleScan = async (sourceId: string) => {
    setScanningId(sourceId);
    setError(null);
    try {
      const result = await api.scanSource(sourceId);
      setScanResults((prev) => ({ ...prev, [sourceId]: result }));
    } catch (e) {
      setError(String(e));
    } finally {
      setScanningId(null);
    }
  };

  const handleScanAll = async () => {
    setScanningAll(true);
    setError(null);
    try {
      const results = await api.scanAllSources();
      const map: Record<string, IngestResult> = {};
      for (const r of results) {
        map[r.sourceId] = r;
      }
      setScanResults((prev) => ({ ...prev, ...map }));
    } catch (e) {
      setError(String(e));
    } finally {
      setScanningAll(false);
    }
  };

  if (loading) return <LoadingSpinner className="py-24" />;

  return (
    <div className="mx-auto max-w-3xl p-6">
      <div className="mb-6 flex items-center justify-between">
        <h2 className="text-xl font-semibold">Sources</h2>
        <div className="flex gap-2">
          <button
            onClick={handleScanAll}
            disabled={scanningAll || sources.length === 0}
            className="rounded-md bg-gray-700 px-3 py-1.5 text-xs font-medium text-gray-200 transition hover:bg-gray-600 disabled:opacity-50"
          >
            {scanningAll ? "Scanning…" : "Scan All"}
          </button>
          <button
            onClick={() => setShowAddForm(!showAddForm)}
            className="rounded-md bg-blue-600 px-3 py-1.5 text-xs font-medium text-white transition hover:bg-blue-500"
          >
            + Add Source
          </button>
        </div>
      </div>

      {/* Error */}
      {error && (
        <div className="mb-4 rounded-md border border-red-800 bg-red-900/30 px-4 py-2 text-sm text-red-300">
          {error}
        </div>
      )}

      {/* Add source form */}
      {showAddForm && (
        <div className="mb-6 rounded-lg border border-gray-700 bg-gray-800/60 p-4">
          <h3 className="mb-3 text-sm font-medium">Add a Source Directory</h3>

          <label className="mb-1 block text-xs text-gray-400">Root Path</label>
          <input
            type="text"
            value={formPath}
            onChange={(e) => setFormPath(e.target.value)}
            placeholder="C:\\Users\\you\\notes  or  /home/user/notes"
            className="mb-3 w-full rounded border border-gray-700 bg-gray-900 px-3 py-2 text-sm text-gray-100 placeholder-gray-600"
          />

          <label className="mb-1 block text-xs text-gray-400">
            Include Globs (comma-separated)
          </label>
          <input
            type="text"
            value={formInclude}
            onChange={(e) => setFormInclude(e.target.value)}
            placeholder="**/*.md, **/*.txt"
            className="mb-3 w-full rounded border border-gray-700 bg-gray-900 px-3 py-2 text-sm text-gray-100 placeholder-gray-600"
          />

          <label className="mb-1 block text-xs text-gray-400">
            Exclude Globs (comma-separated)
          </label>
          <input
            type="text"
            value={formExclude}
            onChange={(e) => setFormExclude(e.target.value)}
            placeholder="**/node_modules/**, **/.git/**"
            className="mb-3 w-full rounded border border-gray-700 bg-gray-900 px-3 py-2 text-sm text-gray-100 placeholder-gray-600"
          />

          <div className="flex justify-end gap-2">
            <button
              onClick={() => setShowAddForm(false)}
              className="rounded px-3 py-1.5 text-xs text-gray-400 hover:text-gray-200"
            >
              Cancel
            </button>
            <button
              onClick={handleAdd}
              disabled={adding || !formPath.trim()}
              className="rounded bg-blue-600 px-3 py-1.5 text-xs font-medium text-white hover:bg-blue-500 disabled:opacity-50"
            >
              {adding ? "Adding…" : "Add Source"}
            </button>
          </div>
        </div>
      )}

      {/* Sources list */}
      {sources.length === 0 ? (
        <p className="py-12 text-center text-sm text-gray-500">
          No sources added yet. Click &ldquo;Add Source&rdquo; to get started.
        </p>
      ) : (
        <div className="space-y-3">
          {sources.map((source) => (
            <div
              key={source.id}
              className="rounded-lg border border-gray-700 bg-gray-800/60 p-4"
            >
              <div className="flex items-start justify-between gap-2">
                <div className="min-w-0 flex-1">
                  <p className="truncate font-mono text-sm text-gray-200">
                    {source.rootPath}
                  </p>
                  <div className="mt-1 flex flex-wrap gap-1">
                    {source.includeGlobs.map((g, i) => (
                      <span
                        key={i}
                        className="rounded bg-green-900/40 px-1.5 py-0.5 text-xs text-green-300"
                      >
                        {g}
                      </span>
                    ))}
                    {source.excludeGlobs.map((g, i) => (
                      <span
                        key={`e-${i}`}
                        className="rounded bg-red-900/40 px-1.5 py-0.5 text-xs text-red-300"
                      >
                        ✕ {g}
                      </span>
                    ))}
                  </div>
                  <p className="mt-1 text-xs text-gray-500">
                    Watch: {source.watchEnabled ? "on" : "off"}
                  </p>
                </div>

                <div className="flex shrink-0 gap-1">
                  <button
                    onClick={() => handleScan(source.id)}
                    disabled={scanningId === source.id}
                    className="rounded bg-gray-700 px-2 py-1 text-xs text-gray-300 transition hover:bg-gray-600 disabled:opacity-50"
                  >
                    {scanningId === source.id ? "Scanning…" : "Scan"}
                  </button>
                  <button
                    onClick={() => handleDelete(source.id)}
                    className="rounded bg-gray-700 px-2 py-1 text-xs text-red-400 transition hover:bg-red-900/40"
                  >
                    Delete
                  </button>
                </div>
              </div>

              {/* Scan result */}
              {scanResults[source.id] && (
                <div className="mt-3 rounded border border-gray-700 bg-gray-900/50 px-3 py-2 text-xs text-gray-400">
                  <ScanResultSummary result={scanResults[source.id]} />
                </div>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function ScanResultSummary({ result }: { result: IngestResult }) {
  return (
    <div className="flex flex-wrap gap-x-4 gap-y-1">
      <span>Scanned: {result.filesScanned}</span>
      <span className="text-green-400">Added: {result.filesAdded}</span>
      <span className="text-blue-400">Updated: {result.filesUpdated}</span>
      <span>Skipped: {result.filesSkipped}</span>
      {result.filesFailed > 0 && (
        <span className="text-red-400">Failed: {result.filesFailed}</span>
      )}
      {result.errors.length > 0 && (
        <div className="mt-1 w-full">
          {result.errors.map((err, i) => (
            <p key={i} className="text-red-400">
              {err}
            </p>
          ))}
        </div>
      )}
    </div>
  );
}
