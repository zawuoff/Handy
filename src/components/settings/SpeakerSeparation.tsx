import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import { toast } from "sonner";
import { commands } from "@/bindings";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { useSettings } from "../../hooks/useSettings";

interface DiarizerDownloadPayload {
  progress: number;
  done: boolean;
  error: string | null;
}

interface SpeakerSeparationProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

/**
 * Meeting speaker-separation toggle. Enabling it downloads the local
 * diarizer model (~140 MB) once; progress is shown in the description.
 */
export const SpeakerSeparation: React.FC<SpeakerSeparationProps> = ({
  descriptionMode,
  grouped = false,
}) => {
  const { getSetting, updateSetting, isUpdating } = useSettings();
  const { t } = useTranslation();
  const enabled = (getSetting("diarization_enabled") as boolean) ?? false;
  const [downloadPercent, setDownloadPercent] = useState<number | null>(null);

  useEffect(() => {
    const unlisten = listen<DiarizerDownloadPayload>(
      "diarizer-download",
      (event) => {
        if (event.payload.done) {
          setDownloadPercent(null);
          if (event.payload.error) {
            toast.error(t("settings.speakerSeparation.downloadFailed"));
          }
        } else {
          setDownloadPercent(Math.round(event.payload.progress * 100));
        }
      },
    );
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [t]);

  const handleChange = async (value: boolean) => {
    updateSetting("diarization_enabled", value);
    if (!value) return;
    const ready = await commands.isDiarizerReady();
    if (ready.status === "ok" && !ready.data) {
      setDownloadPercent(0);
      commands.downloadDiarizerModel();
    }
  };

  return (
    <ToggleSwitch
      checked={enabled}
      onChange={handleChange}
      isUpdating={isUpdating("diarization_enabled")}
      label={t("settings.speakerSeparation.title")}
      description={
        downloadPercent !== null
          ? t("settings.speakerSeparation.downloading", {
              percent: downloadPercent,
            })
          : t("settings.speakerSeparation.description")
      }
      descriptionMode={downloadPercent !== null ? "inline" : descriptionMode}
      grouped={grouped}
    />
  );
};
