import React from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { useSettings } from "../../hooks/useSettings";

interface MeetingRadarProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

export const MeetingRadar: React.FC<MeetingRadarProps> = ({
  descriptionMode,
  grouped = false,
}) => {
  const { getSetting, updateSetting, isUpdating } = useSettings();
  const { t } = useTranslation();
  const enabled = (getSetting("meeting_radar_enabled") as boolean) ?? false;

  return (
    <ToggleSwitch
      checked={enabled}
      onChange={(value) => updateSetting("meeting_radar_enabled", value)}
      isUpdating={isUpdating("meeting_radar_enabled")}
      label={t("settings.meetingRadar.title")}
      description={t("settings.meetingRadar.description")}
      descriptionMode={descriptionMode}
      grouped={grouped}
    />
  );
};
