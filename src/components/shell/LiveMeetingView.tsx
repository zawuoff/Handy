import React, { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Captions, CaptionsOff } from "lucide-react";
import { commands, events } from "@/bindings";
import { formatElapsed, type MeetingUiState } from "./useMeetingState";

// Survives view remounts within one meeting (e.g. window hidden and shown):
// keyed by the meeting's start time so a new meeting starts blank.
const notesCache = { key: 0, text: "" };

/**
 * Full-window view shown while a meeting session is recording, mirroring
 * mockup 3 of the noted_design file: a notes pad on the left where the user
 * can jot freely (fed to note generation as the anchor), and a contrasting
 * live-transcript rail on the right.
 */
export const LiveMeetingView: React.FC<{ meeting: MeetingUiState }> = ({
  meeting,
}) => {
  const { t } = useTranslation();
  const [now, setNow] = useState(Date.now());
  const [committed, setCommitted] = useState("");
  const [tentative, setTentative] = useState("");
  const [captionsOn, setCaptionsOn] = useState(meeting.captionsVisible);
  const [notes, setNotes] = useState(() =>
    notesCache.key === meeting.startedAtMs ? notesCache.text : "",
  );
  const scrollRef = useRef<HTMLDivElement>(null);
  const notesRef = useRef<HTMLTextAreaElement>(null);

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

  // Grow the notes pad with its content instead of scrolling inside itself.
  useEffect(() => {
    const el = notesRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight}px`;
  }, [notes]);

  // Push jottings to the session (debounced) so they ride along with the
  // transcript when the meeting ends — even if this window closes first.
  useEffect(() => {
    notesCache.key = meeting.startedAtMs;
    notesCache.text = notes;
    const timer = setTimeout(() => {
      commands.setLiveMeetingNotes(notes);
    }, 500);
    return () => clearTimeout(timer);
  }, [notes, meeting.startedAtMs]);

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
            onClick={async () => {
              // Flush jottings still sitting in the debounce window before
              // the session ends and hands them to the save pipeline.
              await commands.setLiveMeetingNotes(notes);
              await commands.toggleMeetingSession();
            }}
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

      <div className="flex-1 overflow-hidden flex min-h-0">
        {/* Notes pad — the user's own jottings, optional. */}
        <div className="flex-1 overflow-y-auto min-w-0">
          <div className="max-w-[60ch] mx-auto px-8 py-8 flex flex-col gap-3">
            <h1 className="font-serif text-[26px] leading-tight font-medium">
              {t("live.notesTitle")}
            </h1>
            <p className="text-[12.5px] text-faint">{t("live.notesHint")}</p>
            <textarea
              ref={notesRef}
              className="w-full bg-transparent resize-none outline-none border-none text-[15px] leading-7 text-text placeholder:text-faint placeholder:italic min-h-48 overflow-hidden"
              value={notes}
              placeholder={t("live.notesPlaceholder")}
              onChange={(e) => setNotes(e.target.value)}
              spellCheck={false}
              autoFocus
            />
          </div>
        </div>

        {/* Live transcript rail, visually separated from the pad. */}
        <div className="w-[340px] shrink-0 border-s border-border bg-card flex flex-col">
          <div className="flex items-center gap-2 px-5 pt-5 pb-3 shrink-0">
            <span className="w-1.5 h-1.5 rounded-full bg-record animate-pulse" />
            <span className="text-[11px] font-semibold tracking-[0.08em] uppercase text-faint">
              {t("live.liveTranscript")}
            </span>
          </div>
          <div ref={scrollRef} className="flex-1 overflow-y-auto px-5 pb-4">
            {meeting.streaming ? (
              hasText ? (
                <p className="text-[13.5px] leading-6 whitespace-pre-wrap select-text">
                  <span className="text-muted">{committed}</span>
                  {tentative && (
                    <span className="text-faint"> {tentative}</span>
                  )}
                </p>
              ) : (
                <p className="text-sm text-faint italic">
                  {t("live.listening")}
                </p>
              )
            ) : (
              <p className="text-sm text-faint italic">
                {t("live.noStreamingModel")}
              </p>
            )}
          </div>
          <div className="px-5 py-3 border-t border-border shrink-0">
            <span className="text-[11px] text-faint">{t("live.privacy")}</span>
          </div>
        </div>
      </div>
    </div>
  );
};
