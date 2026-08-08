import { useCallback, useEffect, useMemo, useState } from 'react';
import { CalendarDays, Check, FolderKanban, Network, Plus, RefreshCw, Save } from 'lucide-react';
import { toast } from 'sonner';
import * as api from '../../lib/api';
import type { Project } from '../../types/project';
import { useTranslation } from '../../i18n';
import { Badge } from '../ui/Badge';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { Modal } from '../ui/Modal';

interface ProjectWorkspacePanelProps {
  projectId: string | null;
  open: boolean;
  onClose: () => void;
}

type WorkspaceTab = 'overview' | 'knowledge' | 'timeline';

export function ProjectWorkspacePanel({ projectId, open, onClose }: ProjectWorkspacePanelProps) {
  const { t } = useTranslation();
  const [activeTab, setActiveTab] = useState<WorkspaceTab>('overview');
  const [project, setProject] = useState<Project | null>(null);
  const [workspace, setWorkspace] = useState<api.ProjectWorkspaceSnapshot | null>(null);
  const [narrative, setNarrative] = useState<api.NarrativeEvidencePlan | null>(null);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [brief, setBrief] = useState('');
  const [instructions, setInstructions] = useState('');
  const [query, setQuery] = useState('');
  const [subject, setSubject] = useState('');
  const [predicate, setPredicate] = useState('');
  const [object, setObject] = useState('');

  const load = useCallback(async (nextQuery?: string) => {
    if (!projectId) return;
    setLoading(true);
    try {
      const [nextProject, nextWorkspace, nextNarrative] = await Promise.all([
        api.getProject(projectId),
        api.getProjectWorkspace(projectId, nextQuery),
        api.getProjectNarrative(projectId, nextQuery?.trim() || ''),
      ]);
      setProject(nextProject);
      setWorkspace(nextWorkspace);
      setNarrative(nextNarrative);
      setBrief(nextProject.description);
      setInstructions(nextProject.systemPrompt);
    } catch (error) {
      toast.error(String(error));
    } finally {
      setLoading(false);
    }
  }, [projectId]);

  useEffect(() => {
    if (open) void load();
  }, [open, load]);

  const saveOverview = async () => {
    if (!projectId || !project) return;
    setSaving(true);
    try {
      const updated = await api.updateProject(projectId, {
        description: brief.trim(),
        systemPrompt: instructions.trim(),
      });
      setProject(updated);
      setWorkspace((current) => current ? {
        ...current,
        brief: updated.description,
        instructions: updated.systemPrompt,
      } : current);
      toast.success(t('project.workspaceSaved'));
    } catch (error) {
      toast.error(String(error));
    } finally {
      setSaving(false);
    }
  };

  const addClaim = async () => {
    if (!projectId || !subject.trim() || !predicate.trim() || !object.trim()) return;
    setSaving(true);
    try {
      await api.createProjectKnowledgeClaim(projectId, {
        subject: subject.trim(),
        predicate: predicate.trim(),
        object: object.trim(),
        reviewState: 'accepted',
        confidence: 1,
        provenance: { kind: 'explicit_user_workspace_entry' },
        sourceRef: `project:${projectId}`,
      });
      setSubject('');
      setPredicate('');
      setObject('');
      await load(query);
    } catch (error) {
      toast.error(String(error));
    } finally {
      setSaving(false);
    }
  };

  const acceptClaim = async (claim: api.KnowledgeClaim) => {
    try {
      await api.reviewProjectKnowledgeClaim(claim.id, 'accepted');
      await load(query);
    } catch (error) {
      toast.error(String(error));
    }
  };

  const acceptedClaims = useMemo(
    () => narrative?.supportingClaims ?? [],
    [narrative],
  );

  const tabs: Array<{ id: WorkspaceTab; label: string; icon: typeof FolderKanban }> = [
    { id: 'overview', label: t('project.workspaceOverview'), icon: FolderKanban },
    { id: 'knowledge', label: t('project.workspaceKnowledge'), icon: Network },
    { id: 'timeline', label: t('project.workspaceTimeline'), icon: CalendarDays },
  ];

  return (
    <Modal
      open={open && !!projectId}
      onClose={onClose}
      title={t('project.workspaceTitle')}
      footer={
        <Button variant="ghost" size="sm" onClick={onClose}>
          {t('common.close')}
        </Button>
      }
    >
      <div className="space-y-4">
        <div className="grid grid-cols-3 gap-1 rounded-lg bg-surface-1 p-1" role="tablist">
          {tabs.map(({ id, label, icon: Icon }) => (
            <button
              key={id}
              type="button"
              role="tab"
              aria-selected={activeTab === id}
              onClick={() => setActiveTab(id)}
              className={`flex items-center justify-center gap-1.5 rounded-md px-2 py-1.5 text-xs transition-colors ${
                activeTab === id
                  ? 'bg-surface-0 text-text-primary shadow-sm'
                  : 'text-text-tertiary hover:text-text-secondary'
              }`}
            >
              <Icon size={13} />
              <span>{label}</span>
            </button>
          ))}
        </div>

        {activeTab === 'overview' && (
          <div className="space-y-4">
            <div className="grid grid-cols-2 gap-2">
              <WorkspaceMetric label={t('project.workspaceEpisodes')} value={workspace?.episodes.length ?? 0} />
              <WorkspaceMetric label={t('project.workspaceEvents')} value={workspace?.events.length ?? 0} />
            </div>
            <label className="block space-y-1.5">
              <span className="text-xs font-medium text-text-secondary">{t('project.workspaceBrief')}</span>
              <textarea
                value={brief}
                onChange={(event) => setBrief(event.target.value)}
                placeholder={t('project.workspaceBriefPlaceholder')}
                className="min-h-20 w-full resize-y rounded-md border border-border bg-surface-0 px-3 py-2 text-sm text-text-primary outline-none placeholder:text-text-tertiary focus:border-accent"
              />
            </label>
            <label className="block space-y-1.5">
              <span className="text-xs font-medium text-text-secondary">{t('project.workspaceInstructions')}</span>
              <textarea
                value={instructions}
                onChange={(event) => setInstructions(event.target.value)}
                placeholder={t('project.workspaceInstructionsPlaceholder')}
                className="min-h-28 w-full resize-y rounded-md border border-border bg-surface-0 px-3 py-2 text-sm text-text-primary outline-none placeholder:text-text-tertiary focus:border-accent"
              />
              <span className="block text-[11px] leading-4 text-text-tertiary">
                {t('project.workspaceInstructionsHint')}
              </span>
            </label>
            <div className="flex justify-end">
              <Button
                variant="primary"
                size="sm"
                icon={<Save size={13} />}
                loading={saving}
                disabled={!project || (brief === project.description && instructions === project.systemPrompt)}
                onClick={() => void saveOverview()}
              >
                {t('common.save')}
              </Button>
            </div>
          </div>
        )}

        {activeTab === 'knowledge' && (
          <div className="space-y-4">
            <div className="rounded-md border border-border bg-surface-1 p-3">
              <div className="grid grid-cols-1 gap-2 sm:grid-cols-3">
                <Input value={subject} onChange={(event) => setSubject(event.target.value)} placeholder={t('project.workspaceClaimSubject')} />
                <Input value={predicate} onChange={(event) => setPredicate(event.target.value)} placeholder={t('project.workspaceClaimPredicate')} />
                <Input value={object} onChange={(event) => setObject(event.target.value)} placeholder={t('project.workspaceClaimObject')} />
              </div>
              <div className="mt-3 flex justify-end">
                <Button
                  variant="primary"
                  size="sm"
                  icon={<Plus size={13} />}
                  loading={saving}
                  disabled={!subject.trim() || !predicate.trim() || !object.trim()}
                  onClick={() => void addClaim()}
                >
                  {t('project.workspaceAddClaim')}
                </Button>
              </div>
            </div>
            <div className="flex gap-2">
              <Input
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                onKeyDown={(event) => { if (event.key === 'Enter') void load(query); }}
                placeholder={t('project.workspaceKnowledgeQuery')}
              />
              <Button
                variant="ghost"
                size="sm"
                iconOnly
                icon={<RefreshCw size={13} />}
                aria-label={t('project.memoryRefresh')}
                title={t('project.memoryRefresh')}
                loading={loading}
                onClick={() => void load(query)}
              />
            </div>
            <div className="max-h-72 space-y-3 overflow-auto pr-1">
              <ClaimGroup label={t('project.workspaceAcceptedClaims')} claims={acceptedClaims} />
              <ClaimGroup
                label={t('project.workspaceReviewQueue')}
                claims={narrative?.openQuestions ?? []}
                onAccept={acceptClaim}
              />
              {acceptedClaims.length === 0 && (narrative?.openQuestions.length ?? 0) === 0 && (
                <div className="rounded-md border border-dashed border-border px-3 py-8 text-center text-xs text-text-tertiary">
                  {t('project.workspaceKnowledgeEmpty')}
                </div>
              )}
            </div>
          </div>
        )}

        {activeTab === 'timeline' && (
          <div className="max-h-[28rem] space-y-4 overflow-auto pr-1">
            {(workspace?.events.length ?? 0) === 0 ? (
              <div className="rounded-md border border-dashed border-border px-3 py-8 text-center text-xs text-text-tertiary">
                {t('project.workspaceTimelineEmpty')}
              </div>
            ) : workspace?.events.map((event) => (
              <article key={event.id} className="rounded-md border border-border bg-surface-1 p-3">
                <div className="flex items-center gap-2">
                  <Badge variant="default">{event.eventType}</Badge>
                  <span className="min-w-0 flex-1 truncate text-xs font-medium text-text-primary">{event.title}</span>
                  <span className="text-[10px] text-text-tertiary">{event.reviewState}</span>
                </div>
                <p className="mt-2 text-xs leading-5 text-text-secondary">{event.summary}</p>
                <div className="mt-2 font-mono text-[10px] text-text-tertiary">{event.turnId ?? event.id}</div>
              </article>
            ))}
            {workspace?.episodes.map((episode) => (
              <article key={episode.id} className="rounded-md border border-border/70 p-3">
                <p className="text-xs leading-5 text-text-secondary">{episode.summary}</p>
                <div className="mt-2 truncate font-mono text-[10px] text-text-tertiary">{episode.evidence.join(' · ')}</div>
              </article>
            ))}
          </div>
        )}
      </div>
    </Modal>
  );
}

function WorkspaceMetric({ label, value }: { label: string; value: number }) {
  return (
    <div className="rounded-md border border-border bg-surface-1 p-3">
      <div className="text-[10px] uppercase tracking-wide text-text-tertiary">{label}</div>
      <div className="mt-1 text-lg font-semibold text-text-primary">{value}</div>
    </div>
  );
}

function ClaimGroup({
  label,
  claims,
  onAccept,
}: {
  label: string;
  claims: api.KnowledgeClaim[];
  onAccept?: (claim: api.KnowledgeClaim) => Promise<void>;
}) {
  const { t } = useTranslation();
  if (claims.length === 0) return null;
  return (
    <section className="space-y-2">
      <div className="text-xs font-medium text-text-secondary">{label}</div>
      {claims.map((claim) => (
        <article key={claim.id} className="rounded-md border border-border bg-surface-1 p-3">
          <div className="flex items-start gap-2">
            <p className="min-w-0 flex-1 text-xs leading-5 text-text-secondary">
              <span className="font-medium text-text-primary">{claim.subject}</span>{' '}
              {claim.predicate} {claim.object}
            </p>
            {onAccept && (
              <Button
                variant="ghost"
                size="sm"
                icon={<Check size={12} />}
                onClick={() => void onAccept(claim)}
              >
                {t('project.workspaceAcceptClaim')}
              </Button>
            )}
          </div>
          <div className="mt-2 truncate font-mono text-[10px] text-text-tertiary">
            {claim.evidenceRefs.join(' · ') || claim.reviewState}
          </div>
        </article>
      ))}
    </section>
  );
}
