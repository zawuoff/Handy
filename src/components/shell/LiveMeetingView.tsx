import React, { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Captions, CaptionsOff } from "lucide-react";
import { commands, events } from "@/bindings";
import { formatElapsed, type MeetingUiState } from "./useMeetingState";

/**
 * Full-window view shown while a meeting session is recording: a live
 * transcript fed by the streaming model (when available), a ticking timer,
 * and the stop control. Mirrors mockup 3 of the noted_design file, minus the
 * per-speaker labels (single-channel capture has no speaker identity yet).
 */
export const LiveMeetingView: React.FC<{ meeting: MeetingUiState }> = ({
  meeting,
}) => {
  const { t } = useTranslation();
  const [now, setNow] = useState(Date.now());
  const [committed, setCommitted] = useState("");
  const [tentative, setTentative] = useState("");
  const [captionsOn, setCaptionsOn] = useState(meeting.captionsVisible);
  const scrollRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, []);

  useEffect(() => {
    const unlisten = events.streamTextEvent.listen((event) => {
      setCommitted(event.payload.committed);
      setTentative(event.payload.tentative);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // Keep the transcript pinned to the latest words.
  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [committed, tentative]);

  const toggleCaptions = async () => {
    const result = await commands.toggleMeetingCaptions();
    if (result.status === "ok") setCaptionsOn((prev) => !prev);
  };

  const hasText = committed.trim().length > 0 || tentative.trim().length > 0;

  return (
    <div className="h-full flex flex-col bg-background">
      <div className="flex items-center justify-between px-4 py-3 border-b border-border shrink-0">
        <span className="text-[13px] text-muted">{t("live.title")}</span>
        <div className="flex items-center gap-2 rounded-full border border-border-strong bg-card px-4 py-1.5">
          <span className="w-2 h-2 rounded-full bg-record animate-pulse shrink-0" />
          <span className="text-[13px] font-medium text-text">
            {t("live.recording")} · {formatElapsed(meeting.startedAtMs, now)}
          </span>
          <span className="w-px h-4 bg-border-strong mx-1" />
          <button
            className="text-[13px] font-medium text-muted hover:text-text cursor-pointer"
            onClick={() => commands.toggleMeetingSession()}
          >
            {t("live.stop")}
          </button>
        </div>
        {meeting.streaming ? (
          <button
            className="flex items-center gap-1.5 text-[13px] text-muted hover:text-text cursor-pointer"
            onClick={toggleCaptions}
          >
            {captionsOn ? <CaptionsOff size={15} /> : <Captions size={15} />}
            {t("live.captions")}
          </button>
        ) : (
          <span className="w-20" />
        )}
      </div>

      <div className="flex-1 overflow-hidden flex flex-col max-w-3xl w-full mx-auto px-8">
        <span className="text-xs font-semibold tracking-[0.05em] uppercase text-faint pt-6 pb-3 shrink-0">
          {t("live.liveTranscript")}
        </span>
        <div ref={scrollRef} className="flex-1 overflow-y-auto pb-6">
          {meeting.streaming ? (
            hasText ? (
              <p className="text-[15px] leading-7 whitespace-pre-wrap select-text">
                <span className="text-text">{committed}</span>
                {tentative && <span className="text-muted"> {tentative}</span>}
              </p>
            ) : (
              <p className="text-sm text-faint italic">{t("live.listening")}</p>
            )
          ) : (
            <p className="text-sm text-faint italic">
              {t("live.noStreamingModel")}
            </p>
          )}
        </div>
      </div>

      <div className="px-4 py-3 text-center shrink-0">
        <span className="text-[11.5px] text-faint">{t("live.privacy")}</span>
      </div>
    </div>
  );
};
