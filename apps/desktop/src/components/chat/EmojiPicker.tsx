import { useState, useRef, useEffect, useCallback, lazy, Suspense } from "react";
import { Smile } from "lucide-react";
import { motion, AnimatePresence } from "framer-motion";
import { useTranslation } from "../../i18n";
import { useTheme } from "../../lib/ThemeProvider";

const LazyPicker = lazy(async () => {
  const [{ default: data }, { default: Picker }] = await Promise.all([
    import("@emoji-mart/data"),
    import("@emoji-mart/react"),
  ]);
  // Wrap the default export so React.lazy gets { default: Component }
  return {
    default: (props: Record<string, unknown>) => (
      <Picker data={data} {...props} />
    ),
  };
});

interface EmojiPickerProps {
  onEmojiSelect: (emoji: string) => void;
  disabled?: boolean;
}

export function EmojiPicker({ onEmojiSelect, disabled }: EmojiPickerProps) {
  const { t } = useTranslation();
  const { theme } = useTheme();
  const [open, setOpen] = useState(false);
  const containerRef = useRef<HTMLDivElement>(null);

  const handleClickOutside = useCallback(
    (e: MouseEvent) => {
      if (
        containerRef.current &&
        !containerRef.current.contains(e.target as Node)
      ) {
        setOpen(false);
      }
    },
    [],
  );

  useEffect(() => {
    if (open) {
      document.addEventListener("mousedown", handleClickOutside);
    }
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, [open, handleClickOutside]);

  return (
    <div ref={containerRef} className="relative">
      <motion.button
        whileTap={{ scale: 0.95 }}
        onClick={() => setOpen((prev) => !prev)}
        disabled={disabled}
        className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg text-text-tertiary transition-colors duration-fast ease-out cursor-pointer hover:bg-surface-2 hover:text-text-secondary disabled:pointer-events-none disabled:opacity-40"
        aria-label={t("chat.insertEmoji")}
        type="button"
      >
        <Smile className="h-4 w-4" />
      </motion.button>

      <AnimatePresence>
        {open && (
          <motion.div
            initial={{ opacity: 0, y: 8, scale: 0.95 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 8, scale: 0.95 }}
            transition={{ duration: 0.15 }}
            className="absolute bottom-12 right-0 z-50"
          >
            <Suspense
              fallback={
                <div className="flex h-[350px] w-[352px] items-center justify-center rounded-lg bg-surface-1 shadow-lg">
                  <Smile className="h-6 w-6 animate-pulse text-text-tertiary" />
                </div>
              }
            >
              <LazyPicker
                onEmojiSelect={(emoji: { native: string }) => {
                  onEmojiSelect(emoji.native);
                  setOpen(false);
                }}
                theme={theme === "light" ? "light" : "dark"}
                previewPosition="none"
                skinTonePosition="search"
                maxFrequentRows={2}
                perLine={8}
              />
            </Suspense>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
