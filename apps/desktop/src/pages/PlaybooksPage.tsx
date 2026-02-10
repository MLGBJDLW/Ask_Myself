import { useState, useEffect, useCallback } from "react";
import * as api from "../lib/api";
import type { Playbook, PlaybookCitation } from "../types";
import { LoadingSpinner } from "../components/LoadingSpinner";

export function PlaybooksPage() {
  const [playbooks, setPlaybooks] = useState<Playbook[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // ── Create form ───────────────────────────────────────────────────────
  const [showCreateForm, setShowCreateForm] = useState(false);
  const [formTitle, setFormTitle] = useState("");
  const [formDesc, setFormDesc] = useState("");
  const [formQuery, setFormQuery] = useState("");
  const [creating, setCreating] = useState(false);

  // ── Detail view ───────────────────────────────────────────────────────
  const [selectedPlaybook, setSelectedPlaybook] = useState<Playbook | null>(null);
  const [citations, setCitations] = useState<PlaybookCitation[]>([]);
  const [loadingCitations, setLoadingCitations] = useState(false);

  const loadPlaybooks = useCallback(async () => {
    try {
      const list = await api.listPlaybooks();
      setPlaybooks(list);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    loadPlaybooks();
  }, [loadPlaybooks]);

  const handleCreate = async () => {
    if (!formTitle.trim()) return;
    setCreating(true);
    setError(null);
    try {
      await api.createPlaybook(formTitle.trim(), formDesc.trim(), formQuery.trim());
      setFormTitle("");
      setFormDesc("");
      setFormQuery("");
      setShowCreateForm(false);
      await loadPlaybooks();
    } catch (e) {
      setError(String(e));
    } finally {
      setCreating(false);
    }
  };

  const handleDelete = async (playbookId: string) => {
    setError(null);
    try {
      await api.deletePlaybook(playbookId);
      if (selectedPlaybook?.id === playbookId) {
        setSelectedPlaybook(null);
        setCitations([]);
      }
      await loadPlaybooks();
    } catch (e) {
      setError(String(e));
    }
  };

  const handleSelect = async (playbook: Playbook) => {
    setSelectedPlaybook(playbook);
    setLoadingCitations(true);
    try {
      const cits = await api.listCitations(playbook.id);
      setCitations(cits);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoadingCitations(false);
    }
  };

  const handleRemoveCitation = async (citationId: string) => {
    setError(null);
    try {
      await api.removeCitation(citationId);
      setCitations((prev) => prev.filter((c) => c.id !== citationId));
    } catch (e) {
      setError(String(e));
    }
  };

  if (loading) return <LoadingSpinner className="py-24" />;

  return (
    <div className="mx-auto max-w-4xl p-6">
      <div className="mb-6 flex items-center justify-between">
        <h2 className="text-xl font-semibold">Playbooks</h2>
        <button
          onClick={() => setShowCreateForm(!showCreateForm)}
          className="rounded-md bg-blue-600 px-3 py-1.5 text-xs font-medium text-white transition hover:bg-blue-500"
        >
          + Create Playbook
        </button>
      </div>

      {/* Error */}
      {error && (
        <div className="mb-4 rounded-md border border-red-800 bg-red-900/30 px-4 py-2 text-sm text-red-300">
          {error}
        </div>
      )}

      {/* Create form */}
      {showCreateForm && (
        <div className="mb-6 rounded-lg border border-gray-700 bg-gray-800/60 p-4">
          <h3 className="mb-3 text-sm font-medium">New Playbook</h3>

          <label className="mb-1 block text-xs text-gray-400">Title</label>
          <input
            type="text"
            value={formTitle}
            onChange={(e) => setFormTitle(e.target.value)}
            className="mb-3 w-full rounded border border-gray-700 bg-gray-900 px-3 py-2 text-sm text-gray-100"
            placeholder="My research topic"
          />

          <label className="mb-1 block text-xs text-gray-400">Description</label>
          <input
            type="text"
            value={formDesc}
            onChange={(e) => setFormDesc(e.target.value)}
            className="mb-3 w-full rounded border border-gray-700 bg-gray-900 px-3 py-2 text-sm text-gray-100"
            placeholder="Brief description of this playbook"
          />

          <label className="mb-1 block text-xs text-gray-400">
            Base Query (optional)
          </label>
          <input
            type="text"
            value={formQuery}
            onChange={(e) => setFormQuery(e.target.value)}
            className="mb-4 w-full rounded border border-gray-700 bg-gray-900 px-3 py-2 text-sm text-gray-100"
            placeholder="Search query this playbook was born from"
          />

          <div className="flex justify-end gap-2">
            <button
              onClick={() => setShowCreateForm(false)}
              className="rounded px-3 py-1.5 text-xs text-gray-400 hover:text-gray-200"
            >
              Cancel
            </button>
            <button
              onClick={handleCreate}
              disabled={creating || !formTitle.trim()}
              className="rounded bg-blue-600 px-3 py-1.5 text-xs font-medium text-white hover:bg-blue-500 disabled:opacity-50"
            >
              {creating ? "Creating…" : "Create"}
            </button>
          </div>
        </div>
      )}

      <div className="flex gap-6">
        {/* Playbook list */}
        <div className="w-1/2 space-y-2">
          {playbooks.length === 0 ? (
            <p className="py-12 text-center text-sm text-gray-500">
              No playbooks yet.
            </p>
          ) : (
            playbooks.map((pb) => (
              <button
                key={pb.id}
                onClick={() => handleSelect(pb)}
                className={`w-full rounded-lg border p-3 text-left transition ${
                  selectedPlaybook?.id === pb.id
                    ? "border-blue-600 bg-blue-900/20"
                    : "border-gray-700 bg-gray-800/60 hover:border-gray-600"
                }`}
              >
                <div className="flex items-start justify-between">
                  <div className="min-w-0 flex-1">
                    <p className="font-medium text-sm text-gray-200 truncate">
                      {pb.title}
                    </p>
                    {pb.description && (
                      <p className="mt-0.5 text-xs text-gray-400 truncate">
                        {pb.description}
                      </p>
                    )}
                    <p className="mt-1 text-xs text-gray-600">
                      {new Date(pb.createdAt).toLocaleDateString()}
                    </p>
                  </div>
                  <button
                    onClick={(e) => {
                      e.stopPropagation();
                      handleDelete(pb.id);
                    }}
                    className="ml-2 shrink-0 rounded px-1.5 py-0.5 text-xs text-red-400 hover:bg-red-900/40"
                    title="Delete playbook"
                  >
                    ✕
                  </button>
                </div>
              </button>
            ))
          )}
        </div>

        {/* Detail panel */}
        <div className="w-1/2">
          {selectedPlaybook ? (
            <div>
              <h3 className="mb-1 text-lg font-semibold">
                {selectedPlaybook.title}
              </h3>
              {selectedPlaybook.description && (
                <p className="mb-4 text-sm text-gray-400">
                  {selectedPlaybook.description}
                </p>
              )}

              <h4 className="mb-2 text-xs font-medium uppercase tracking-wider text-gray-500">
                Citations ({citations.length})
              </h4>

              {loadingCitations ? (
                <LoadingSpinner className="py-8" />
              ) : citations.length === 0 ? (
                <p className="text-sm text-gray-500">
                  No citations yet. Save evidence cards from search results.
                </p>
              ) : (
                <div className="space-y-2">
                  {citations.map((cit) => (
                    <div
                      key={cit.id}
                      className="rounded-md border border-gray-700 bg-gray-800/40 p-3"
                    >
                      <div className="flex items-start justify-between">
                        <p className="text-xs font-mono text-gray-400">
                          Chunk: {cit.chunkId.slice(0, 8)}…
                        </p>
                        <button
                          onClick={() => handleRemoveCitation(cit.id)}
                          className="text-xs text-red-400 hover:text-red-300"
                          title="Remove citation"
                        >
                          ✕
                        </button>
                      </div>
                      {cit.annotation && (
                        <p className="mt-1 text-sm text-gray-300">
                          {cit.annotation}
                        </p>
                      )}
                    </div>
                  ))}
                </div>
              )}
            </div>
          ) : (
            <div className="flex h-full items-center justify-center">
              <p className="text-sm text-gray-500">
                Select a playbook to view its citations
              </p>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
