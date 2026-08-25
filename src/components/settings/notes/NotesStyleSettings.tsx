import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import { useSettings } from "@/hooks/useSettings";
import { Button } from "../../ui/Button";
import { Textarea } from "../../ui/Textarea";

/**
 * Settings page for how meeting notes get written: the instructions sent to
 * the configured model together with each transcript.
 */
export const NotesStyleSettings: React.FC = () => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, resetSetting } = useSettings();
  const stored = (getSetting("meeting_notes_prompt") as string) ?? "";
  const [draft, setDraft] = useState<string | null>(null);
  const value = draft ?? stored;

  const save = () => {
    if (draft !== null && draft.trim() && draft !== stored) {
      updateSetting("meeting_notes_prompt", draft);
    }
    setDraft(null);
  };

  return (
    <div className="max-w-3xl w-full mx-auto flex flex-col gap-3">
      <div className="rounded-xl border border-border bg-card p-4 flex flex-col gap-2">
        <h3 className="text-sm font-semibold">{t("notes.promptTitle")}</h3>
        <p className="text-xs text-muted">{t("notes.promptDescription")}</p>
        <Textarea
          value={value}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={save}
          rows={14}
        />
        <Button
          variant="secondary"
          size="sm"
          className="self-start"
          onClick={() => {
            setDraft(null);
            resetSetting("meeting_notes_prompt");
          }}
        >
          {t("notes.promptReset")}
        </Button>
      </div>
    </div>
  );
};
