import { useMemo, useState } from 'react';
import { open } from '@tauri-apps/plugin-dialog';
import { AnimatePresence, motion } from 'framer-motion';
import { AlertTriangle, CheckCircle2, FileArchive, FolderOpen, Loader2, PackagePlus, ShieldAlert, X } from 'lucide-react';
import { useTranslation } from '../../i18n';
import * as api from '../../lib/api';
import type { DiscoveredSkillBundle, Skill } from '../../types/extensions';
import { Badge } from '../ui/Badge';
import { Button } from '../ui/Button';

interface SkillInstallerProps {
  skills: Skill[];
  onInstalled?: () => void;
}

export function SkillInstaller({ skills, onInstalled }: SkillInstallerProps) {
  const { t } = useTranslation();
  const [openInstaller, setOpenInstaller] = useState(false);
  const [source, setSource] = useState<string | null>(null);
  const [preview, setPreview] = useState<DiscoveredSkillBundle[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [replaceExisting, setReplaceExisting] = useState(false);
  const [acceptBlocked, setAcceptBlocked] = useState(false);

  const installedNames = useMemo(
    () => new Set(skills.filter((skill) => !skill.builtin).map((skill) => skill.canonicalName.toLocaleLowerCase())),
    [skills],
  );
  const builtinNames = useMemo(
    () => new Set(skills.filter((skill) => skill.builtin).map((skill) => skill.canonicalName.toLocaleLowerCase())),
    [skills],
  );
  const conflicts = useMemo(
    () => preview.filter((skill) => installedNames.has(skill.name.toLocaleLowerCase())),
    [installedNames, preview],
  );
  const builtinConflicts = useMemo(
    () => preview.filter((skill) => builtinNames.has(skill.name.toLocaleLowerCase())),
    [builtinNames, preview],
  );
  const warnings = preview.flatMap((skill) => skill.warnings.map((warning) => ({ skill: skill.name, ...warning })));
  const hasBlockedWarnings = warnings.some((warning) => warning.severity === 'block');
  const canInstall = preview.length > 0
    && builtinConflicts.length === 0
    && (!conflicts.length || replaceExisting)
    && (!hasBlockedWarnings || acceptBlocked)
    && !busy;

  const reset = () => {
    setSource(null);
    setPreview([]);
    setError(null);
    setReplaceExisting(false);
    setAcceptBlocked(false);
  };

  const close = () => {
    if (busy) return;
    setOpenInstaller(false);
    reset();
  };

  const chooseSource = async (directory: boolean) => {
    setError(null);
    try {
      const selected = await open({
        directory,
        multiple: false,
        ...(directory
          ? {}
          : {
              filters: [{
                name: t('settings.skillInstallSupportedFiles'),
                extensions: ['skill', 'zip', 'md'],
              }],
            }),
      });
      if (!selected || Array.isArray(selected)) return;
      setBusy(true);
      const result = await api.inspectSkillInstallSource(selected);
      setSource(selected);
      setPreview(result);
      setReplaceExisting(false);
      setAcceptBlocked(false);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };

  const install = async () => {
    if (!source || !canInstall) return;
    setBusy(true);
    setError(null);
    try {
      await api.installSkillsFromSource(source, replaceExisting, acceptBlocked);
      onInstalled?.();
      setOpenInstaller(false);
      reset();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <Button
        variant="secondary"
        size="sm"
        icon={<PackagePlus size={14} />}
        onClick={() => setOpenInstaller(true)}
      >
        {t('settings.skillInstall')}
      </Button>

      <AnimatePresence>
        {openInstaller && (
          <motion.div
            className="fixed inset-0 z-60 flex items-center justify-center bg-black/55 p-4 backdrop-blur-sm"
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            onMouseDown={(event) => event.target === event.currentTarget && close()}
          >
            <motion.section
              role="dialog"
              aria-modal="true"
              aria-labelledby="skill-installer-title"
              data-testid="skill-installer"
              className="flex max-h-[min(760px,88vh)] w-full max-w-2xl flex-col overflow-hidden rounded-2xl border border-border/80 bg-surface-1 shadow-2xl"
              initial={{ opacity: 0, y: 18, scale: 0.97 }}
              animate={{ opacity: 1, y: 0, scale: 1 }}
              exit={{ opacity: 0, y: 12, scale: 0.98 }}
              transition={{ type: 'spring', stiffness: 360, damping: 30 }}
            >
              <header className="flex items-start justify-between gap-4 border-b border-border bg-linear-to-r from-accent/10 via-surface-1 to-surface-1 px-5 py-4">
                <div>
                  <div className="mb-1 flex items-center gap-2 text-accent">
                    <PackagePlus size={18} />
                    <h2 id="skill-installer-title" className="text-base font-semibold text-text-primary">
                      {t('settings.skillInstallTitle')}
                    </h2>
                  </div>
                  <p className="text-xs leading-5 text-text-secondary">
                    {t('settings.skillInstallDescription')}
                  </p>
                </div>
                <button
                  type="button"
                  aria-label={t('common.close')}
                  onClick={close}
                  className="rounded-lg p-1.5 text-text-tertiary transition hover:bg-surface-3 hover:text-text-primary"
                >
                  <X size={16} />
                </button>
              </header>

              <div className="flex-1 space-y-4 overflow-y-auto p-5">
                <div className="grid gap-3 sm:grid-cols-2">
                  <button
                    type="button"
                    onClick={() => chooseSource(false)}
                    disabled={busy}
                    className="group rounded-xl border border-border bg-surface-2 p-4 text-left transition hover:-translate-y-0.5 hover:border-accent/50 hover:bg-accent/5 disabled:opacity-50"
                  >
                    <FileArchive className="mb-3 text-accent transition-transform group-hover:scale-110" size={22} />
                    <p className="text-sm font-medium text-text-primary">{t('settings.skillInstallPackage')}</p>
                    <p className="mt-1 text-xs text-text-tertiary">.skill, .zip, SKILL.md</p>
                  </button>
                  <button
                    type="button"
                    onClick={() => chooseSource(true)}
                    disabled={busy}
                    className="group rounded-xl border border-border bg-surface-2 p-4 text-left transition hover:-translate-y-0.5 hover:border-accent/50 hover:bg-accent/5 disabled:opacity-50"
                  >
                    <FolderOpen className="mb-3 text-accent transition-transform group-hover:scale-110" size={22} />
                    <p className="text-sm font-medium text-text-primary">{t('settings.skillInstallFolder')}</p>
                    <p className="mt-1 text-xs text-text-tertiary">{t('settings.skillInstallFolderHint')}</p>
                  </button>
                </div>

                {busy && preview.length === 0 && (
                  <div className="flex items-center justify-center gap-2 rounded-xl border border-border bg-surface-2 py-8 text-sm text-text-secondary">
                    <Loader2 size={16} className="animate-spin text-accent" />
                    {t('settings.skillInstallInspecting')}
                  </div>
                )}

                {source && preview.length > 0 && (
                  <div className="space-y-3" data-testid="skill-install-preview">
                    <div className="flex items-center gap-2 text-xs text-text-tertiary">
                      <CheckCircle2 size={14} className="text-success" />
                      <span className="truncate" title={source}>{source}</span>
                    </div>
                    {preview.map((skill) => {
                      const conflict = installedNames.has(skill.name.toLocaleLowerCase());
                      return (
                        <article key={skill.skillFile} className="rounded-xl border border-border bg-surface-2 p-3">
                          <div className="flex items-start justify-between gap-3">
                            <div className="min-w-0">
                              <p className="truncate text-sm font-semibold text-text-primary">{skill.name}</p>
                              <p className="mt-0.5 text-xs text-text-secondary">{skill.description}</p>
                            </div>
                            <div className="flex shrink-0 gap-1">
                              {conflict && <Badge variant="default">{t('settings.skillInstallUpdate')}</Badge>}
                              <Badge variant="default">{t('settings.skillInstallResourceCount', { count: String(skill.resources.length) })}</Badge>
                            </div>
                          </div>
                        </article>
                      );
                    })}
                  </div>
                )}

                {conflicts.length > 0 && (
                  <label className="flex cursor-pointer items-start gap-3 rounded-xl border border-warning/35 bg-warning/8 p-3 text-xs text-text-secondary">
                    <input
                      type="checkbox"
                      checked={replaceExisting}
                      onChange={(event) => setReplaceExisting(event.target.checked)}
                      className="mt-0.5 accent-accent"
                    />
                    <span>
                      <span className="block font-medium text-text-primary">{t('settings.skillInstallReplaceTitle')}</span>
                      {t('settings.skillInstallReplaceDescription', { names: conflicts.map((skill) => skill.name).join(', ') })}
                    </span>
                  </label>
                )}

                {builtinConflicts.length > 0 && (
                  <div role="alert" className="flex items-start gap-2 rounded-xl border border-danger/35 bg-danger/8 p-3 text-xs text-danger">
                    <ShieldAlert size={15} className="mt-0.5 shrink-0" />
                    {t('settings.skillInstallBuiltinConflict', { names: builtinConflicts.map((skill) => skill.name).join(', ') })}
                  </div>
                )}

                {warnings.length > 0 && (
                  <div className="rounded-xl border border-warning/35 bg-warning/8 p-3">
                    <div className="mb-2 flex items-center gap-2 text-xs font-medium text-warning">
                      <AlertTriangle size={14} />
                      {t('settings.skillInstallWarnings', { count: String(warnings.length) })}
                    </div>
                    <ul className="max-h-36 space-y-1 overflow-y-auto text-xs text-text-secondary">
                      {warnings.map((warning, index) => (
                        <li key={`${warning.skill}-${warning.code}-${index}`} className="flex gap-2">
                          <span className={warning.severity === 'block' ? 'text-danger' : 'text-warning'}>•</span>
                          <span><strong>{warning.skill}</strong>: {warning.message}</span>
                        </li>
                      ))}
                    </ul>
                    {hasBlockedWarnings && (
                      <label className="mt-3 flex cursor-pointer items-start gap-2 border-t border-warning/25 pt-3 text-xs text-text-secondary">
                        <input
                          type="checkbox"
                          checked={acceptBlocked}
                          onChange={(event) => setAcceptBlocked(event.target.checked)}
                          className="mt-0.5 accent-danger"
                        />
                        <ShieldAlert size={14} className="shrink-0 text-danger" />
                        {t('settings.skillInstallRiskConfirm')}
                      </label>
                    )}
                  </div>
                )}

                {error && (
                  <div role="alert" className="rounded-xl border border-danger/30 bg-danger/8 px-3 py-2 text-xs text-danger">
                    {error}
                  </div>
                )}
              </div>

              <footer className="flex items-center justify-end gap-2 border-t border-border bg-surface-2/70 px-5 py-3">
                <Button variant="ghost" size="sm" onClick={close}>{t('common.cancel')}</Button>
                <Button
                  variant="primary"
                  size="sm"
                  loading={busy && preview.length > 0}
                  disabled={!canInstall}
                  onClick={install}
                >
                  {t('settings.skillInstallConfirm', { count: String(preview.length) })}
                </Button>
              </footer>
            </motion.section>
          </motion.div>
        )}
      </AnimatePresence>
    </>
  );
}
