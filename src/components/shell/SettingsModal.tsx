import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Cog,
  FlaskConical,
  Info,
  NotebookPen,
  Settings2,
  Sparkles,
  X,
  Cpu,
} from "lucide-react";
import Footer from "../footer";
import {
  GeneralSettings,
  AdvancedSettings,
  DebugSettings,
  AboutSettings,
  PostProcessingSettings,
  ModelsSettings,
  NotesStyleSettings,
} from "../settings";
import { useSettings } from "@/hooks/useSettings";

type SettingsTab =
  | "general"
  | "notes"
  | "models"
  | "advanced"
  | "postprocessing"
  | "debug"
  | "about";

const TABS: {
  id: SettingsTab;
  labelKey: string;
  icon: React.ComponentType<{ size?: number | string; className?: string }>;
  component: React.ComponentType;
  enabled: (settings: any) => boolean;
}[] = [
  {
    id: "general",
    labelKey: "sidebar.general",
    icon: Settings2,
    component: GeneralSettings,
    enabled: () => true,
  },
  {
    id: "notes",
    labelKey: "sidebar.notes",
    icon: NotebookPen,
    component: NotesStyleSettings,
    enabled: () => true,
  },
  {
    id: "models",
    labelKey: "sidebar.models",
    icon: Cpu,
    component: ModelsSettings,
    enabled: () => true,
  },
  {
    id: "advanced",
    labelKey: "sidebar.advanced",
    icon: Cog,
    component: AdvancedSettings,
    enabled: () => true,
  },
  {
    id: "postprocessing",
    labelKey: "sidebar.postProcessing",
    icon: Sparkles,
    component: PostProcessingSettings,
    enabled: (settings) => settings?.post_process_enabled ?? false,
  },
  {
    id: "debug",
    labelKey: "sidebar.debug",
    icon: FlaskConical,
    component: DebugSettings,
    enabled: (settings) => settings?.debug_mode ?? false,
  },
  {
    id: "about",
    labelKey: "sidebar.about",
    icon: Info,
    component: AboutSettings,
    enabled: () => true,
  },
];

/**
 * The settings panel from mockup 4 of noted_design: a centered modal with a
 * SETTINGS rail on the left and the active page on the right. Hosts all of
 * the original settings pages unchanged.
 */
export const SettingsModal: React.FC<{
  open: boolean;
  onOpenChange: (open: boolean) => void;
}> = ({ open, onOpenChange }) => {
  const { t } = useTranslation();
  const { settings } = useSettings();
  const [tab, setTab] = useState<SettingsTab>("general");

  useEffect(() => {
    if (!open) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onOpenChange(false);
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [open, onOpenChange]);

  if (!open) return null;

  const availableTabs = TABS.filter((entry) => entry.enabled(settings));
  const active =
    availableTabs.find((entry) => entry.id === tab) ?? availableTabs[0];
  const ActiveComponent = active.component;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-6"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onOpenChange(false);
      }}
    >
      <div
        className="flex w-[min(94vw,58rem)] h-[min(85vh,620px)] rounded-2xl border border-border-strong bg-background shadow-2xl overflow-hidden"
        role="dialog"
        aria-modal="true"
        aria-label={t("shell.settings")}
      >
        <div className="flex flex-col w-48 shrink-0 border-e border-border bg-surface p-3 gap-0.5 overflow-y-auto">
          <span className="px-2.5 pt-1 pb-2.5 text-[11px] font-semibold tracking-[0.08em] uppercase text-faint">
            {t("shell.settings")}
          </span>
          {availableTabs.map((entry) => {
            const Icon = entry.icon;
            const isActive = entry.id === active.id;
            return (
              <button
                key={entry.id}
                className={`flex items-center gap-2.5 px-2.5 py-[7px] rounded-[7px] text-[13px] cursor-pointer transition-colors text-start ${
                  isActive
                    ? "bg-card text-text font-medium"
                    : "text-muted hover:bg-card2 hover:text-text"
                }`}
                onClick={() => setTab(entry.id)}
              >
                <Icon size={15} className="shrink-0" />
                <span className="truncate">{t(entry.labelKey)}</span>
              </button>
            );
          })}
        </div>

        <div className="flex-1 min-w-0 flex flex-col">
          <div className="flex items-center justify-between px-7 pt-5 pb-3 shrink-0">
            <h2 className="font-serif text-xl font-medium text-text">
              {t(active.labelKey)}
            </h2>
            <button
              className="p-1.5 rounded-md text-muted hover:text-text cursor-pointer"
              title={t("common.close")}
              onClick={() => onOpenChange(false)}
            >
              <X size={18} />
            </button>
          </div>
          <div className="flex-1 min-w-0 overflow-y-auto overflow-x-hidden px-7 pb-5">
            <ActiveComponent />
            <div className="pt-5">
              <Footer />
            </div>
          </div>
        </div>
      </div>
    </div>
  );
};
