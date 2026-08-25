import React from "react";
import { useTranslation } from "react-i18next";
import { History, Home, Mic, NotebookPen, Settings } from "lucide-react";
import ModelSelector from "../model-selector";
import { useOsType } from "@/hooks/useOsType";
import { useSettings } from "@/hooks/useSettings";
import { formatKeyCombination } from "@/lib/utils/keyboard";

export type ShellView = "home" | "notes" | "history";

const NavItem: React.FC<{
  icon: React.ReactNode;
  label: string;
  active: boolean;
  onClick: () => void;
}> = ({ icon, label, active, onClick }) => (
  <button
    className={`flex items-center gap-2.5 w-full px-2.5 py-[7px] rounded-[7px] text-[13px] cursor-pointer transition-colors text-start ${
      active
        ? "bg-card text-text font-medium"
        : "text-muted hover:bg-card2 hover:text-text"
    }`}
    onClick={onClick}
  >
    <span className="shrink-0">{icon}</span>
    <span className="truncate">{label}</span>
  </button>
);

export const ShellSidebar: React.FC<{
  view: ShellView;
  onViewChange: (view: ShellView) => void;
  onOpenSettings: () => void;
}> = ({ view, onViewChange, onOpenSettings }) => {
  const { t } = useTranslation();
  const { getSetting } = useSettings();
  const osType = useOsType();

  const bindings = getSetting("bindings") ?? {};
  const dictationShortcut = bindings["transcribe"]?.current_binding
    ? formatKeyCombination(bindings["transcribe"].current_binding, osType)
    : "";
  const pushToTalk = (getSetting("push_to_talk") as boolean) ?? true;

  return (
    <div className="flex flex-col w-56 h-full shrink-0 bg-surface border-e border-border px-3 py-4 gap-0.5">
      <div className="px-2.5 pb-3">
        <span className="font-serif text-lg font-medium text-text">
          {t("shell.appName")}
        </span>
      </div>

      <NavItem
        icon={<Home size={15} />}
        label={t("shell.home")}
        active={view === "home"}
        onClick={() => onViewChange("home")}
      />
      <NavItem
        icon={<NotebookPen size={15} />}
        label={t("sidebar.notes")}
        active={view === "notes"}
        onClick={() => onViewChange("notes")}
      />
      <NavItem
        icon={<History size={15} />}
        label={t("sidebar.history")}
        active={view === "history"}
        onClick={() => onViewChange("history")}
      />

      <div className="flex-1" />

      <div className="flex items-center gap-2.5 rounded-lg border border-border bg-background px-2.5 py-2 mb-1">
        <Mic size={14} className="shrink-0 text-accent" />
        <div className="flex flex-col min-w-0">
          <span className="text-xs font-medium text-text">
            {t("shell.dictationReady")}
          </span>
          {dictationShortcut && (
            <span className="text-[10.5px] text-faint truncate">
              {pushToTalk
                ? t("shell.dictationHintHold", { keys: dictationShortcut })
                : t("shell.dictationHintPress", { keys: dictationShortcut })}
            </span>
          )}
        </div>
      </div>

      <div className="px-1 py-1">
        <ModelSelector />
      </div>

      <NavItem
        icon={<Settings size={15} />}
        label={t("shell.settings")}
        active={false}
        onClick={onOpenSettings}
      />
    </div>
  );
};
