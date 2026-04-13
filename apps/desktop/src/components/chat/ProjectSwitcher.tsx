import { useState, useEffect, useRef, useCallback } from 'react';
import { FolderOpen, Plus, ChevronDown, Check } from 'lucide-react';
import { useTranslation } from '../../i18n';
import type { Project, CreateProjectInput } from '../../types/project';
import * as api from '../../lib/api';

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
  const dropdownRef = useRef<HTMLDivElement>(null);

  const loadProjects = useCallback(async () => {
    try {
      const list = await api.listProjects();
      setProjects(list);
    } catch {
      // silent
    }
  }, []);

  useEffect(() => {
    loadProjects();
  }, [loadProjects]);

  // Close dropdown on outside click
  useEffect(() => {
    if (!open) return;
    const handler = (e: MouseEvent) => {
      if (dropdownRef.current && !dropdownRef.current.contains(e.target as Node)) {
        setOpen(false);
        setCreating(false);
        setNewName('');
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
    try {
      const input: CreateProjectInput = { name: trimmed };
      const created = await api.createProject(input);
      setNewName('');
      setCreating(false);
      await loadProjects();
      onProjectChange(created.id);
      setOpen(false);
    } catch {
      // silent
    }
  };

  return (
    <div className="relative" ref={dropdownRef}>
      <button
        onClick={() => setOpen((v) => !v)}
        className="w-full flex items-center gap-2 px-3 py-2 text-xs font-medium
          text-text-primary hover:bg-surface-2 rounded-md transition-colors cursor-pointer"
      >
        <FolderOpen className="h-3.5 w-3.5 text-text-tertiary shrink-0" />
        <span className="truncate flex-1 text-left">
          {activeProject ? (
            <>
              {activeProject.icon && <span className="mr-1">{activeProject.icon}</span>}
              {activeProject.name}
            </>
          ) : (
            t('project.allConversations')
          )}
        </span>
        <ChevronDown className={`h-3 w-3 text-text-tertiary shrink-0 transition-transform ${open ? 'rotate-180' : ''}`} />
      </button>

      {open && (
        <div className="absolute left-0 right-0 top-full mt-1 z-50 bg-surface-2 border border-border
          rounded-lg shadow-lg py-1 text-xs max-h-64 overflow-y-auto">
          {/* All Conversations */}
          <button
            onClick={() => handleSelect(null)}
            className="w-full flex items-center gap-2 px-3 py-1.5 hover:bg-surface-3 text-text-secondary
              hover:text-text-primary transition-colors cursor-pointer"
          >
            <FolderOpen className="h-3 w-3 shrink-0" />
            <span className="flex-1 text-left">{t('project.allConversations')}</span>
            {activeProjectId === null && <Check className="h-3 w-3 text-accent shrink-0" />}
          </button>

          {/* Divider */}
          {projects.length > 0 && <div className="border-t border-border my-1" />}

          {/* Project list */}
          {projects.map((p) => (
            <button
              key={p.id}
              onClick={() => handleSelect(p.id)}
              className="w-full flex items-center gap-2 px-3 py-1.5 hover:bg-surface-3 text-text-secondary
                hover:text-text-primary transition-colors cursor-pointer"
            >
              <span className="h-3 w-3 shrink-0 text-center text-[10px]">
                {p.icon || '📁'}
              </span>
              <span className="flex-1 text-left truncate">{p.name}</span>
              {p.id === activeProjectId && <Check className="h-3 w-3 text-accent shrink-0" />}
            </button>
          ))}

          {/* Divider */}
          <div className="border-t border-border my-1" />

          {/* Create new */}
          {creating ? (
            <div className="px-3 py-1.5">
              <input
                autoFocus
                value={newName}
                onChange={(e) => setNewName(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') handleCreate();
                  if (e.key === 'Escape') { setCreating(false); setNewName(''); }
                }}
                placeholder={t('project.namePlaceholder')}
                className="w-full bg-surface-0 border border-border rounded px-2 py-1 text-xs
                  text-text-primary placeholder:text-text-tertiary outline-none focus:border-accent"
              />
            </div>
          ) : (
            <button
              onClick={() => setCreating(true)}
              className="w-full flex items-center gap-2 px-3 py-1.5 hover:bg-surface-3 text-accent
                hover:text-accent-hover transition-colors cursor-pointer"
            >
              <Plus className="h-3 w-3 shrink-0" />
              <span>{t('project.createNew')}</span>
            </button>
          )}
        </div>
      )}
    </div>
  );
}
