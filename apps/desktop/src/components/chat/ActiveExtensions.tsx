import { useState, useEffect, useCallback, useRef } from 'react';
import { motion, AnimatePresence, useReducedMotion } from 'framer-motion';
import { Plug, Zap, AlertTriangle } from 'lucide-react';
import { useTranslation } from '../../i18n';
import * as api from '../../lib/api';
import { getSoftDropdownMotion } from '../../lib/uiMotion';
import type { McpServer, McpToolInfo, Skill } from '../../types/extensions';

/* ------------------------------------------------------------------ */
/*  Types                                                              */
/* ------------------------------------------------------------------ */

interface ServerWithTools {
  server: McpServer;
  tools: McpToolInfo[];
  error?: string;
}

/* ------------------------------------------------------------------ */
/*  Dropdown chip (matches SystemPromptEditor trigger + dropdown)      */
/* ------------------------------------------------------------------ */

function ChipDropdown({
  icon,
  label,
  active,
  children,
}: {
  icon: React.ReactNode;
  label: string;
  active: boolean;
  children: React.ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const shouldReduceMotion = useReducedMotion();
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const handler = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener('mousedown', handler);
    return () => document.removeEventListener('mousedown', handler);
  }, [open]);

  return (
    <div ref={ref} className="relative inline-block">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        className="
          flex items-center gap-1.5 px-2 py-1 rounded-md text-xs
          hover:bg-surface-2 transition-colors duration-fast
          border border-transparent hover:border-border
        "
      >
        {icon}
        <span className={active ? 'text-accent' : 'text-text-tertiary'}>{label}</span>
        {active && <span className="w-1.5 h-1.5 rounded-full bg-accent shrink-0" />}
      </button>

      <AnimatePresence>
        {open && (
          <motion.div
            {...getSoftDropdownMotion(!!shouldReduceMotion)}
            className="
              absolute left-0 top-full mt-1 z-50
              w-64 rounded-lg border border-border bg-surface-1 shadow-lg
              max-h-60 overflow-y-auto
            "
          >
            {children}
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

/* ------------------------------------------------------------------ */
/*  Component                                                          */
/* ------------------------------------------------------------------ */

export function ActiveExtensions({ conversationId }: { conversationId?: string }) {
  const { t } = useTranslation();
  const copy = {
    unavailable: t('chat.extensions.unavailable'),
    toolCount: (count: number) => t('settings.extensions.toolCount', { count }),
  };
  const [serversWithTools, setServersWithTools] = useState<ServerWithTools[]>([]);
  const [skills, setSkills] = useState<Skill[]>([]);
  const [loaded, setLoaded] = useState(false);

  const loadData = useCallback(async () => {
    try {
      const [allServers, allSkills] = await Promise.all([
        api.listMcpServers(),
        api.listActiveSkills(),
      ]);

      const enabled = allServers.filter((s) => s.enabled);
      const withTools = await Promise.all(
        enabled.map(async (server) => {
          try {
            const tools = await api.listMcpTools(server.id);
            return { server, tools };
          } catch (err: unknown) {
            const msg = err instanceof Error ? err.message : String(err);
            return { server, tools: [], error: msg };
          }
        }),
      );

      setServersWithTools(withTools);
      setSkills(allSkills);
    } catch {
      // Silently fail — extensions info is non-critical
    } finally {
      setLoaded(true);
    }
  }, []);

  useEffect(() => {
    void loadData();
  }, [loadData, conversationId]);

  if (!loaded) return null;

  const totalTools = serversWithTools.reduce((sum, s) => sum + s.tools.length, 0);
  const hasMcp = serversWithTools.length > 0;
  const hasSkills = skills.length > 0;

  if (!hasMcp && !hasSkills) return null;

  return (
    <>
      {/* MCP chip */}
      {hasMcp && (
        <ChipDropdown
          icon={<Plug size={14} className="text-accent" />}
          label={t('chat.mcpSummary', {
            servers: serversWithTools.length,
            tools: totalTools,
          })}
          active={totalTools > 0}
        >
          <div className="px-3 py-2 border-b border-border">
            <p className="text-xs font-medium text-text-primary">
              {t('chat.mcpSummary', { servers: serversWithTools.length, tools: totalTools })}
            </p>
          </div>
          <div className="p-2 space-y-2">
            {serversWithTools.map(({ server, tools, error }) => (
              <div key={server.id}>
                <div className="flex items-center gap-1.5">
                  <span className="text-xs text-text-secondary font-medium">{server.name}</span>
                  {error ? (
                    <span
                      className="text-[10px] text-danger bg-danger/10 px-1 py-0.5 rounded cursor-help"
                      title={error}
                    >
                      <AlertTriangle size={9} className="inline mr-0.5 -mt-px" />
                      {copy.unavailable}
                    </span>
                  ) : (
                    <span className="text-[10px] text-text-tertiary bg-surface-3 px-1 py-0.5 rounded">
                      {copy.toolCount(tools.length)}
                    </span>
                  )}
                </div>
                {tools.length > 0 && (
                  <ul className="pl-3 mt-0.5 space-y-px">
                    {tools.map((tool) => (
                      <li key={tool.name} className="font-mono text-[10px] text-text-tertiary">
                        • {tool.name}
                      </li>
                    ))}
                  </ul>
                )}
              </div>
            ))}
          </div>
        </ChipDropdown>
      )}

      {/* Skills chip */}
      {hasSkills && (
        <ChipDropdown
          icon={<Zap size={14} className="text-warning" />}
          label={t('chat.skillsSummary', { count: skills.length })}
          active={skills.length > 0}
        >
          <div className="px-3 py-2 border-b border-border">
            <p className="text-xs font-medium text-text-primary">
              {t('chat.skillsSummary', { count: skills.length })}
            </p>
          </div>
          <ul className="p-2 space-y-px">
            {skills.map((skill) => (
              <li key={skill.id} className="text-xs text-text-secondary px-1 py-0.5">
                • {skill.name}
              </li>
            ))}
          </ul>
        </ChipDropdown>
      )}
    </>
  );
}
