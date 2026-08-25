import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Calendar, FileText, Square } from "lucide-react";
import { commands, events, type HistoryEntry } from "@/bindings";
import type { MeetingUiState } from "./useMeetingState";

const dayLabel = (
  timestamp: number,
  locale: string,
  todayLabel: string,
  yesterdayLabel: string,
): string => {
  const date = new Date(timestamp * 1000);
  const now = new Date();
  const startOfDay = (d: Date) =>
    new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const diffDays = Math.round(
    (startOfDay(now) - startOfDay(date)) / 86_400_000,
  );
  if (diffDays === 0) return todayLabel;
  if (diffDays === 1) return yesterdayLabel;
  return new Intl.DateTimeFormat(locale, {
    month: "long",
    day: "numeric",
  }).format(date);
};

export const HomeView: React.FC<{
  meeting: MeetingUiState;
  onOpenNote: (id: number) => void;
}> = ({ meeting, onOpenNote }) => {
  const { t, i18n } = useTranslation();
  const [entries, setEntries] = useState<HistoryEntry[]>([]);

  useEffect(() => {
    commands.getMeetingEntries(null, 20).then((result) => {
      if (result.status === "ok") setEntries(result.data.entries);
    });
    const unlisten = events.historyUpdatePayload.listen((event) => {
      const payload = event.payload;
      if (payload.action === "added" && payload.entry.source === "meeting") {
        setEntries((prev) => [payload.entry, ...prev]);
      } else if (payload.action === "updated") {
        setEntries((prev) =>
          prev.map((e) => (e.id === payload.entry.id ? payload.entry : e)),
        );
      } else if (payload.action === "deleted") {
        setEntries((prev) => prev.filter((e) => e.id !== payload.id));
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const hour = new Date().getHours();
  const greeting =
    hour >= 5 && hour < 12
      ? t("shell.goodMorning")
      : hour >= 12 && hour < 18
        ? t("shell.goodAfternoon")
        : t("shell.goodEvening");

  const dateLine = new Intl.DateTimeFormat(i18n.language, {
    weekday: "long",
    month: "long",
    day: "numeric",
  }).format(new Date());

  const timeOf = (timestamp: number) =>
    new Intl.DateTimeFormat(i18n.language, { timeStyle: "short" }).format(
      new Date(timestamp * 1000),
    );

  let lastGroup = "";

  return (
    <div className="max-w-3xl w-full mx-auto flex flex-col gap-7 pt-6">
      <div className="flex items-end justify-between gap-4">
        <div className="flex flex-col gap-1.5">
          <span className="text-xs font-medium tracking-[0.04em] uppercase text-muted">
            {dateLine}
          </span>
          <h1 className="font-serif text-[34px] leading-10 font-medium text-text">
            {greeting}
          </h1>
        </div>
        <button
          className={`flex items-center gap-2 rounded-[9px] px-4 py-2 text-[13px] font-semibold cursor-pointer transition-opacity hover:opacity-90 ${
            meeting.active
              ? "bg-record text-background"
              : "bg-text text-background"
          }`}
          onClick={() => commands.toggleMeetingSession()}
        >
          {meeting.active ? (
            <>
              <Square size={11} className="shrink-0" fill="currentColor" />
              {t("shell.stopMeeting")}
            </>
          ) : (
            <>
              <span className="w-2 h-2 rounded-full bg-record shrink-0" />
              {t("shell.startMeeting")}
            </>
          )}
        </button>
      </div>

      <div className="flex flex-col">
        <span className="text-xs font-semibold tracking-[0.05em] uppercase text-faint pb-2">
          {t("shell.recentNotes")}
        </span>
        {entries.length === 0 ? (
          <div className="rounded-xl border border-border bg-card px-5 py-6 flex flex-col gap-1">
            <span className="text-sm font-medium text-text">
              {t("notes.empty")}
            </span>
            <span className="text-xs text-faint">{t("notes.emptyHint")}</span>
          </div>
        ) : (
          <div className="flex flex-col gap-0.5">
            {entries.map((entry) => {
              const group = dayLabel(
                entry.timestamp,
                i18n.language,
                t("shell.today"),
                t("shell.yesterday"),
              );
              const header =
                group !== lastGroup ? (
                  <span
                    className="text-xs font-semibold tracking-[0.05em] uppercase text-faint pt-4 pb-2"
                    key={`h-${group}`}
                  >
                    {group}
                  </span>
                ) : null;
              lastGroup = group;
              return (
                <React.Fragment key={entry.id}>
                  {group !== t("shell.today") && header}
                  <button
                    className="flex items-center gap-3 rounded-[9px] px-3 py-[11px] cursor-pointer hover:bg-card2 transition-colors text-start"
                    onClick={() => onOpenNote(entry.id)}
                  >
                    {entry.ai_notes ? (
                      <Calendar size={16} className="shrink-0 text-accent" />
                    ) : (
                      <FileText size={16} className="shrink-0 text-muted" />
                    )}
                    <div className="flex flex-col flex-1 min-w-0 gap-px">
                      <span className="font-serif text-[15px] leading-[18px] font-medium text-text truncate">
                        {entry.title}
                      </span>
                      <span className="text-[11.5px] text-faint truncate">
                        {t("shell.meetingMeta")}
                      </span>
                    </div>
                    {entry.ai_notes && (
                      <span className="shrink-0 rounded-md bg-card px-2 py-0.5 text-[10.5px] font-medium text-accent">
                        {t("shell.enhanced")}
                      </span>
                    )}
                    <span className="shrink-0 w-16 text-end text-[11.5px] text-faint">
                      {timeOf(entry.timestamp)}
                    </span>
                  </button>
                </React.Fragment>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
};
