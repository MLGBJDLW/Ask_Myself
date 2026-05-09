import { useState, useEffect, useRef, useCallback } from 'react';
import { Brain, FolderOpen, Plus, ChevronDown, Check, Pencil, Save, Trash2, X } from 'lucide-react';
import { toast } from 'sonner';
import { useTranslation } from '../../i18n';
import type { Project, CreateProjectInput, UpdateProjectInput } from '../../types/project';
import * as api from '../../lib/api';
import { PROJECT_ICON_OPTIONS, ProjectIcon, getProjectIconOption } from '../../lib/projectIcons';
import { ConfirmDialog } from '../ui/ConfirmDialog';
import { ProjectMemoryPanel } from './ProjectMemoryPanel';

const PROJECT_STORAGE_KEY = 'active-project-id';

function getStoredProjectId(): string | null {
  try {
    return localStorage.getItem(PROJECT_STORAGE_KEY);
  } catch {
    return null;
  }
}

function setStoredProjectId(id: string | null) {
  if (id) {
    localStorage.setItem(PROJECT_STORAGE_KEY, id);
  } else {
    localStorage.removeItem(PROJECT_STORAGE_KEY);
  }
}

interface ProjectSwitcherProps {
  activeProjectId: string | null;
  onProjectChange: (projectId: string | null) => void;
}

export function useActiveProject() {
  const [activeProjectId, setActiveProjectId] = useState<string | null>(getStoredProjectId);

  const setProject = useCallback((id: string | null) => {
    setActiveProjectId(id);
    setStoredProjectId(id);
  }, []);

  return { activeProjectId, setProject };
}

export function ProjectSwitcher({ activeProjectId, onProjectChange }: ProjectSwitcherProps) {
  const { t } = useTranslation();
  const [projects, setProjects] = useState<Project[]>([]);
  const [open, setOpen] = useState(false);
  const [creating, setCreating] = useState(false);
  const [newName, setNewName] = useState('');
  const [newIcon, setNewIcon] = useState(PROJECT_ICON_OPTIONS[0].id);
  const [editingProjectId, setEditingProjectId] = useState<string | null>(null);
  const [editName, setEditName] = useState('');
  const [editIcon, setEditIcon] = useState(PROJECT_ICON_OPTIONS[0].id);
  const [deleteTarget, setDeleteTarget] = useState<Project | null>(null);
  const [projectBusy, setProjectBusy] = useState(false);
  const [showMemoryPanel, setShowMemoryPanel] = useState(false);
  const dropdownRef = useRef<HTMLDivElement>(null);

  const loadProjects = useCallback(async () => {
    try {
      const list = await api.listProjects();
      setProjects(Array.isArray(list) ? list : []);
    } catch {
      // non-critical
    }
  }, []);

  useEffect(() => {
    loadProjects();
  }, [loadProjects]);

  useEffect(() => {
    if (!open) return;
    const handler = (e: MouseEvent) => {
      if (dropdownRef.current && !dropdownRef.current.contains(e.target as Node)) {
        setOpen(false);
        setCreating(false);
        setNewName('');
        setNewIcon(PROJECT_ICON_OPTIONS[0].id);
        setEditingProjectId(null);
      }
    };
    document.addEventListener('mousedown', handler);
    return () => document.removeEventListener('mousedown', handler);
  }, [open]);

  const activeProject = projects.find((p) => p.id === activeProjectId);

  const handleSelect = (id: string | null) => {
    onProjectChange(id);
    setOpen(false);
  };

  const handleCreate = async () => {
    const trimmed = newName.trim();
    if (!trimmed) return;
    setProjectBusy(true);
    try {
      const input: CreateProjectInput = { name: trimmed, icon: newIcon };
      const created = await api.createProject(input);
      setNewName('');
      setNewIcon(PROJECT_ICON_OPTIONS[0].id);
      setCreating(false);
      await loadProjects();
      onProjectChange(created.id);
      toast.success(t('project.created'));
      setOpen(false);
    } catch {
      toast.error(t('common.error'));
    } finally {
      setProjectBusy(false);
    }
  };

  const startEditProject = (project: Project) => {
    setCreating(false);
    setEditingProjectId(project.id);
    setEditName(project.name);
    setEditIcon(getProjectIconOption(project.icon).id);
  };

  const cancelEditProject = () => {
    setEditingProjectId(null);
    setEditName('');
    setEditIcon(PROJECT_ICON_OPTIONS[0].id);
  };

  const handleUpdateProject = async (project: Project) => {
    const trimmed = editName.trim();
    if (!trimmed) return;
    const input: UpdateProjectInput = { name: trimmed, icon: editIcon };
    setProjectBusy(true);
    try {
      const updated = await api.updateProject(project.id, input);
      setProjects((prev) => prev.map((item) => item.id === updated.id ? updated : item));
      cancelEditProject();
      toast.success(t('common.success'));
    } catch {
      toast.error(t('common.error'));
    } finally {
      setProjectBusy(false);
    }
  };

  const handleDeleteProject = async () => {
    if (!deleteTarget) return;
    setProjectBusy(true);
    try {
      await api.deleteProject(deleteTarget.id);
      setProjects((prev) => prev.filter((item) => item.id !== deleteTarget.id));
      if (activeProjectId === deleteTarget.id) {
        onProjectChange(null);
      }
      setDeleteTarget(null);
      setEditingProjectId(null);
      toast.success(t('project.deleted'));
    } catch {
      toast.error(t('common.error'));
    } finally {
      setProjectBusy(false);
    }
  };

  const renderIconPicker = (value: string, onChange: (id: string) => void) => (
    <div className="grid grid-cols-6 gap-1">
      {PROJECT_ICON_OPTIONS.map((option) => {
        const selected = option.id === value;
        const Icon = option.icon;
        return (
          <button
            key={option.id}
            type="button"
            onClick={() => onChange(option.id)}
            className={`flex h-7 w-7 items-center justify-center rounded-md border transition-colors ${
              selected
                ? 'border-accent bg-accent/10 text-accent'
                : 'border-border bg-surface-1 text-text-tertiary hover:bg-surface-3 hover:text-text-primary'
            }`}
            title={option.label}
            aria-label={option.label}
          >
            <Icon size={14} />
          </button>
        );
      })}
    </div>
  );

  const renderProjectRow = (project: Project) => {
    const editing = editingProjectId === project.id;

    if (editing) {
      return (
        <div key={project.id} className="space-y-2 border-t border-border/60 px-3 py-2 first:border-t-0">
          <input
            autoFocus
            value={editName}
            onChange={(e) => setEditName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') void handleUpdateProject(project);
              if (e.key === 'Escape') cancelEditProject();
            }}
            className="w-full rounded border border-border bg-surface-0 px-2 py-1 text-xs text-text-primary outline-none placeholder:text-text-tertiary focus:border-accent"
          />
          {renderIconPicker(editIcon, setEditIcon)}
          <div className="flex justify-end gap-1">
            <button
              type="button"
              onClick={cancelEditProject}
              className="rounded p-1.5 text-text-tertiary hover:bg-surface-3 hover:text-text-primary"
              aria-label={t('common.cancel')}
              title={t('common.cancel')}
            >
              <X size={13} />
            </button>
            <button
              type="button"
              disabled={projectBusy || !editName.trim()}
              onClick={() => void handleUpdateProject(project)}
              className="rounded p-1.5 text-accent hover:bg-accent/10 disabled:opacity-40"
              aria-label={t('common.save')}
              title={t('common.save')}
            >
              <Save size={13} />
            </button>
          </div>
        </div>
      );
    }

    return (
      <div key={project.id} className="group flex items-center gap-1 px-1">
        <button
          onClick={() => handleSelect(project.id)}
          className="flex min-w-0 flex-1 items-center gap-2 rounded-md px-2 py-1.5 text-text-secondary transition-colors hover:bg-surface-3 hover:text-text-primary"
        >
          <ProjectIcon icon={project.icon} className="h-5 w-5" size={12} />
          <span className="flex-1 truncate text-left">{project.name}</span>
          {project.id === activeProjectId && <Check className="h-3 w-3 shrink-0 text-accent" />}
        </button>
        <div className="flex shrink-0 items-center opacity-0 transition-opacity group-hover:opacity-100">
          <button
            type="button"
            onClick={() => startEditProject(project)}
            className="rounded p-1.5 text-text-tertiary hover:bg-accent/10 hover:text-accent"
            aria-label={t('common.edit')}
            title={t('common.edit')}
          >
            <Pencil size={12} />
          </button>
          <button
            type="button"
            onClick={() => setDeleteTarget(project)}
            className="rounded p-1.5 text-text-tertiary hover:bg-danger/10 hover:text-danger"
            aria-label={t('project.delete')}
            title={t('project.delete')}
          >
            <Trash2 size={12} />
          </button>
        </div>
      </div>
    );
  };

  return (
    <div className="relative" ref={dropdownRef}>
      <button
        onClick={() => setOpen((v) => !v)}
        className="flex w-full cursor-pointer items-center gap-2 rounded-md px-3 py-2 text-xs font-medium text-text-primary transition-colors hover:bg-surface-2"
      >
        <FolderOpen className="h-3.5 w-3.5 shrink-0 text-text-tertiary" />
        <span className="min-w-0 flex-1 truncate text-left">
          {activeProject ? (
            <span className="inline-flex min-w-0 items-center gap-1.5">
              <ProjectIcon icon={activeProject.icon} className="h-5 w-5" size={12} />
              <span className="truncate">{activeProject.name}</span>
            </span>
          ) : (
            t('project.allConversations')
          )}
        </span>
        <ChevronDown className={`h-3 w-3 shrink-0 text-text-tertiary transition-transform ${open ? 'rotate-180' : ''}`} />
      </button>

      {open && (
        <div className="absolute left-0 right-0 top-full z-50 mt-1 max-h-96 overflow-y-auto rounded-lg border border-border bg-surface-2 py-1 text-xs shadow-lg">
          <button
            onClick={() => handleSelect(null)}
            className="flex w-full cursor-pointer items-center gap-2 px-3 py-1.5 text-text-secondary transition-colors hover:bg-surface-3 hover:text-text-primary"
          >
            <FolderOpen className="h-3 w-3 shrink-0" />
            <span className="flex-1 text-left">{t('project.allConversations')}</span>
            {activeProjectId === null && <Check className="h-3 w-3 shrink-0 text-accent" />}
          </button>

          {projects.length > 0 && <div className="my-1 border-t border-border" />}
          {projects.map(renderProjectRow)}

          <div className="my-1 border-t border-border" />

          {activeProjectId && (
            <button
              onClick={() => {
                setShowMemoryPanel(true);
                setOpen(false);
              }}
              className="flex w-full cursor-pointer items-center gap-2 px-3 py-1.5 text-text-secondary transition-colors hover:bg-surface-3 hover:text-text-primary"
            >
              <Brain className="h-3 w-3 shrink-0" />
              <span className="flex-1 text-left">Project 记忆</span>
            </button>
          )}

          {creating ? (
            <div className="space-y-2 px-3 py-2">
              <input
                autoFocus
                value={newName}
                onChange={(e) => setNewName(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') void handleCreate();
                  if (e.key === 'Escape') {
                    setCreating(false);
                    setNewName('');
                    setNewIcon(PROJECT_ICON_OPTIONS[0].id);
                  }
                }}
                placeholder={t('project.namePlaceholder')}
                className="w-full rounded border border-border bg-surface-0 px-2 py-1 text-xs text-text-primary outline-none placeholder:text-text-tertiary focus:border-accent"
              />
              {renderIconPicker(newIcon, setNewIcon)}
              <div className="flex justify-end gap-1">
                <button
                  type="button"
                  onClick={() => {
                    setCreating(false);
                    setNewName('');
                    setNewIcon(PROJECT_ICON_OPTIONS[0].id);
                  }}
                  className="rounded p-1.5 text-text-tertiary hover:bg-surface-3 hover:text-text-primary"
                  aria-label={t('common.cancel')}
                  title={t('common.cancel')}
                >
                  <X size={13} />
                </button>
                <button
                  type="button"
                  disabled={projectBusy || !newName.trim()}
                  onClick={() => void handleCreate()}
                  className="rounded p-1.5 text-accent hover:bg-accent/10 disabled:opacity-40"
                  aria-label={t('common.save')}
                  title={t('common.save')}
                >
                  <Save size={13} />
                </button>
              </div>
            </div>
          ) : (
            <button
              onClick={() => {
                setEditingProjectId(null);
                setCreating(true);
              }}
              className="flex w-full cursor-pointer items-center gap-2 px-3 py-1.5 text-accent transition-colors hover:bg-surface-3 hover:text-accent-hover"
            >
              <Plus className="h-3 w-3 shrink-0" />
              <span>{t('project.createNew')}</span>
            </button>
          )}
        </div>
      )}

      <ProjectMemoryPanel
        projectId={activeProjectId}
        open={showMemoryPanel}
        onClose={() => setShowMemoryPanel(false)}
      />
      <ConfirmDialog
        open={!!deleteTarget}
        onClose={() => setDeleteTarget(null)}
        onConfirm={() => { void handleDeleteProject(); }}
        title={t('project.deleteConfirm')}
        message={t('project.deleteConfirmDesc')}
        confirmText={t('common.delete')}
        variant="danger"
        loading={projectBusy}
      />
    </div>
  );
}
