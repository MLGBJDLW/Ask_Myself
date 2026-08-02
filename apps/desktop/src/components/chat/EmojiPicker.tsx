import { useState, useRef, useEffect, lazy, Suspense } from "react";
import { Smile } from "lucide-react";
import { useTranslation } from "../../i18n";
import { useTheme } from "../../lib/ThemeProvider";
import { isLightTheme } from "../../lib/theme";
import {
  NexaPopover,
  NexaPopoverContent,
  NexaPopoverTrigger,
} from "../ui/overlay";

const LazyPicker = lazy(async () => {
  const [{ default: data }, { default: Picker }] = await Promise.all([
    import("@emoji-mart/data"),
    import("emoji-mart").then((module) => ({ default: module.Picker })),
  ]);
  function EmojiMartPicker(props: Record<string, unknown>) {
    const hostRef = useRef<HTMLDivElement>(null);

    useEffect(() => {
      const host = hostRef.current;
      if (!host) return;
      const picker = new Picker({ data, ...props });
      host.replaceChildren(picker as unknown as Node);
      return () => host.replaceChildren();
    }, [props]);

    return <div ref={hostRef} />;
  }

  return {
    default: EmojiMartPicker,
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

  return (
    <NexaPopover open={open} onOpenChange={setOpen}>
      <NexaPopoverTrigger asChild>
      <button
        disabled={disabled}
        className="flex h-8 w-8 shrink-0 items-center justify-center rounded-md text-text-tertiary transition-colors duration-fast ease-out cursor-pointer hover:bg-surface-2 hover:text-text-secondary disabled:pointer-events-none disabled:opacity-40"
        aria-label={t("chat.insertEmoji")}
        type="button"
      >
        <Smile className="h-3.5 w-3.5" />
      </button>
      </NexaPopoverTrigger>

      <NexaPopoverContent side="top" align="end" className="p-0">
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
                theme={isLightTheme(theme) ? "light" : "dark"}
                previewPosition="none"
                skinTonePosition="search"
                emojiButtonColors={["transparent"]}
                maxFrequentRows={2}
                perLine={8}
              />
            </Suspense>
      </NexaPopoverContent>
    </NexaPopover>
  );
}
