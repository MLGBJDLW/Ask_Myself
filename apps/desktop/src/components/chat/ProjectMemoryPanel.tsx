import { useCallback, useEffect, useState } from 'react';
import { Plus, RefreshCw, Trash2 } from 'lucide-react';
import { toast } from 'sonner';
import * as api from '../../lib/api';
import { Badge } from '../ui/Badge';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { Modal } from '../ui/Modal';

interface ProjectMemoryPanelProps {
  projectId: string | null;
  open: boolean;
  onClose: () => void;
}

const KIND_OPTIONS = [
  { value: 'note', label: '笔记' },
  { value: 'fact', label: '事实' },
  { value: 'decision', label: '决策' },
  { value: 'style', label: '风格' },
  { value: 'constraint', label: '约束' },
  { value: 'todo', label: '待办' },
  { value: 'preference', label: '偏好' },
];

export function ProjectMemoryPanel({ projectId, open, onClose }: ProjectMemoryPanelProps) {
  const [memories, setMemories] = useState<api.ProjectMemory[]>([]);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [kind, setKind] = useState('note');
  const [title, setTitle] = useState('');
  const [content, setContent] = useState('');
  const [pinned, setPinned] = useState(false);

  const load = useCallback(async () => {
    if (!projectId) return;
    setLoading(true);
    try {
      setMemories(await api.listProjectMemories(projectId));
    } catch (e) {
      toast.error(String(e));
    } finally {
      setLoading(false);
    }
  }, [projectId]);

  useEffect(() => {
    if (open) void load();
  }, [open, load]);

  const resetForm = () => {
    setKind('note');
    setTitle('');
    setContent('');
    setPinned(false);
  };

  const createMemory = async () => {
    if (!projectId || !content.trim()) return;
    setSaving(true);
    try {
      await api.createProjectMemory(projectId, {
        kind,
        title: title.trim() || null,
        content: content.trim(),
        pinned,
      });
      resetForm();
      await load();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setSaving(false);
    }
  };

  const togglePinned = async (memory: api.ProjectMemory) => {
    try {
      await api.updateProjectMemory(memory.id, { pinned: !memory.pinned });
      await load();
    } catch (e) {
      toast.error(String(e));
    }
  };

  const deleteMemory = async (memory: api.ProjectMemory) => {
    try {
      await api.deleteProjectMemory(memory.id);
      setMemories((prev) => prev.filter((item) => item.id !== memory.id));
    } catch (e) {
      toast.error(String(e));
    }
  };

  return (
    <Modal
      open={open && !!projectId}
      onClose={onClose}
      title="Project 记忆"
      footer={
        <Button variant="ghost" size="sm" onClick={onClose}>
          关闭
        </Button>
      }
    >
      <div className="space-y-4">
        <div className="rounded-md border border-border bg-surface-1 p-3">
          <div className="mb-3 grid grid-cols-1 gap-2 sm:grid-cols-[120px_1fr]">
            <select
              value={kind}
              onChange={(e) => setKind(e.target.value)}
              className="h-9 rounded-md border border-border bg-surface-0 px-2 text-xs text-text-primary outline-none focus:border-accent"
            >
              {KIND_OPTIONS.map((option) => (
                <option key={option.value} value={option.value}>{option.label}</option>
              ))}
            </select>
            <Input
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              placeholder="标题，可选"
            />
          </div>
          <textarea
            value={content}
            onChange={(e) => setContent(e.target.value)}
            placeholder="记录这个 Project 跨对话都应该记住的事实、决策、风格或约束"
            className="min-h-24 w-full resize-y rounded-md border border-border bg-surface-0 px-3 py-2 text-sm text-text-primary outline-none placeholder:text-text-tertiary focus:border-accent"
          />
          <div className="mt-3 flex items-center justify-between gap-3">
            <label className="flex items-center gap-2 text-xs text-text-secondary">
              <input
                type="checkbox"
                checked={pinned}
                onChange={(e) => setPinned(e.target.checked)}
                className="h-4 w-4 rounded border-border text-accent focus:ring-accent/30"
              />
              固定到 prompt
            </label>
            <Button
              variant="primary"
              size="sm"
              icon={<Plus size={14} />}
              onClick={createMemory}
              loading={saving}
              disabled={!content.trim()}
            >
              添加记忆
            </Button>
          </div>
        </div>

        <div className="flex items-center justify-between">
          <div className="text-xs font-medium text-text-secondary">当前 Project 记忆</div>
          <Button
            variant="ghost"
            size="sm"
            iconOnly
            aria-label="刷新项目记忆"
            title="刷新项目记忆"
            icon={<RefreshCw size={13} />}
            loading={loading}
            onClick={load}
          />
        </div>

        <div className="max-h-72 space-y-2 overflow-auto">
          {memories.length === 0 && !loading ? (
            <div className="rounded-md border border-dashed border-border px-3 py-8 text-center text-xs text-text-tertiary">
              还没有项目记忆。
            </div>
          ) : (
            memories.map((memory) => (
              <div key={memory.id} className="rounded-md border border-border bg-surface-1 p-3">
                <div className="mb-2 flex items-center gap-2">
                  <Badge variant={memory.pinned ? 'success' : 'default'}>{memory.kind}</Badge>
                  {memory.title && (
                    <span className="min-w-0 flex-1 truncate text-xs font-medium text-text-primary">
                      {memory.title}
                    </span>
                  )}
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => void togglePinned(memory)}
                  >
                    {memory.pinned ? '取消固定' : '固定'}
                  </Button>
                  <Button
                    variant="danger"
                    size="sm"
                    iconOnly
                    aria-label="删除项目记忆"
                    title="删除项目记忆"
                    icon={<Trash2 size={13} />}
                    onClick={() => void deleteMemory(memory)}
                  />
                </div>
                <p className="whitespace-pre-wrap text-xs leading-5 text-text-secondary">{memory.content}</p>
              </div>
            ))
          )}
        </div>
      </div>
    </Modal>
  );
}
