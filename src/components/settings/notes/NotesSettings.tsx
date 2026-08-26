import React, { useCallback, useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { ask } from "@tauri-apps/plugin-dialog";
import { readFile } from "@tauri-apps/plugin-fs";
import {
  ArrowLeft,
  Calendar,
  Check,
  Copy,
  HardDrive,
  Loader2,
  Mic,
  RotateCcw,
  Sparkles,
  Trash2,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { toast } from "sonner";
import {
  commands,
  events,
  type HistoryEntry,
  type HistoryUpdatePayload,
} from "@/bindings";
import { useOsType } from "@/hooks/useOsType";
import { formatDateTime } from "@/utils/dateFormat";
import { AudioPlayer, AudioPlayerGroup } from "../../ui/AudioPlayer";
import { Button } from "../../ui/Button";

const PAGE_SIZE = 30;

type NotesStatus = "generating" | "done" | "failed";

interface NotesStatusPayload {
  id: number;
  status: NotesStatus;
}

const CopyButton: React.FC<{ text: string; label: string }> = ({
  text,
  label,
}) => {
  const [copied, setCopied] = useState(false);
  return (
    <button
      className="p-1.5 rounded-md flex items-center justify-center transition-colors cursor-pointer text-muted hover:text-text"
      title={label}
      onClick={async () => {
        try {
          await navigator.clipboard.writeText(text);
          setCopied(true);
          setTimeout(() => setCopied(false), 2000);
        } catch (error) {
          console.error("Failed to copy to clipboard:", error);
        }
      }}
    >
      {copied ? (
        <Check width={16} height={16} />
      ) : (
        <Copy width={16} height={16} />
      )}
    </button>
  );
};

const Chip: React.FC<{
  icon: React.ReactNode;
  label: string;
  active?: boolean;
  onClick?: () => void;
  trailing?: React.ReactNode;
}> = ({ icon, label, active, onClick, trailing }) => (
  <span
    className={`inline-flex items-center gap-1.5 rounded-lg border px-3 py-1.5 text-xs font-medium transition-colors ${
      active
        ? "bg-card2 border-border-strong text-text"
        : "bg-card border-border text-muted"
    } ${onClick ? "cursor-pointer hover:text-text hover:border-border-strong" : ""}`}
    onClick={onClick}
    role={onClick ? "button" : undefined}
  >
    {icon}
    {label}
    {trailing}
  </span>
);

const NoteDetail: React.FC<{
  entry: HistoryEntry;
  generating: boolean;
  onBack: () => void;
}> = ({ entry, generating, onBack }) => {
  const { t, i18n } = useTranslation();
  const osType = useOsType();
  const [title, setTitle] = useState(entry.title);
  const [showTranscript, setShowTranscript] = useState(false);
  const [editing, setEditing] = useState(false);
  const docRef = useRef<HTMLTextAreaElement>(null);

  // The note body is one editable document, Notion-style. Until the user
  // touches it, it mirrors the entry (their saved text, falling back to the
  // enhanced notes); the first keystroke takes a local draft that autosaves.
  const derived = entry.user_notes?.trim()
    ? (entry.user_notes ?? "")
    : (entry.ai_notes ?? "");
  const [draft, setDraft] = useState<string | null>(null);
  const doc = draft ?? derived;
  const savedDoc = useRef<string | null>(null);

  useEffect(() => {
    if (draft === null || draft === savedDoc.current) return;
    const timer = setTimeout(async () => {
      savedDoc.current = draft;
      const result = await commands.setHistoryEntryUserNotes(entry.id, draft);
      if (result.status !== "ok") {
        console.error("Failed to save note:", result.error);
      }
    }, 800);
    return () => clearTimeout(timer);
  }, [draft, entry.id]);

  // Grow the document with its content instead of scrolling inside itself.
  useEffect(() => {
    const el = docRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight}px`;
    if (editing) el.focus();
  }, [doc, editing]);

  const saveTitle = async () => {
    const next = title.trim();
    if (!next || next === entry.title) {
      setTitle(entry.title);
      return;
    }
    const result = await commands.setHistoryEntryTitle(entry.id, next);
    if (result.status !== "ok") {
      console.error("Failed to rename note:", result.error);
      setTitle(entry.title);
    }
  };

  const regenerate = async () => {
    const result = await commands.generateMeetingNotes(entry.id);
    if (result.status !== "ok") {
      toast.error(t("notes.failedToast"));
    }
  };

  const handleDelete = async () => {
    const confirmed = await ask(t("notes.deleteConfirm"), {
      title: t("notes.delete"),
      kind: "warning",
    });
    if (!confirmed) return;
    const result = await commands.deleteHistoryEntry(entry.id);
    if (result.status !== "ok") {
      console.error("Failed to delete note:", result.error);
      return;
    }
    onBack();
  };

  const loadAudio = useCallback(async () => {
    try {
      const result = await commands.getAudioFilePath(entry.file_name);
      if (result.status !== "ok") return null;
      if (osType === "linux") {
        const fileData = await readFile(result.data);
        return URL.createObjectURL(new Blob([fileData], { type: "audio/wav" }));
      }
      return convertFileSrc(result.data, "asset");
    } catch (error) {
      console.error("Failed to load audio:", error);
      return null;
    }
  }, [entry.file_name, osType]);

  const hasTranscript = entry.transcription_text.trim().length > 0;

  return (
    <div className="flex flex-col gap-5 w-full">
      <div className="flex items-center gap-2">
        <button
          className="p-1.5 rounded-md text-muted hover:text-text cursor-pointer"
          title={t("notes.back")}
          onClick={onBack}
        >
          <ArrowLeft width={18} height={18} />
        </button>
        <input
          className="flex-1 bg-transparent font-serif text-[32px] leading-tight font-medium outline-none border-b border-transparent focus:border-border-strong min-w-0"
          value={title}
          placeholder={t("notes.titlePlaceholder")}
          onChange={(e) => setTitle(e.target.value)}
          onBlur={saveTitle}
          onKeyDown={(e) => {
            if (e.key === "Enter") (e.target as HTMLInputElement).blur();
          }}
        />
        {doc.trim() && <CopyButton text={doc} label={t("notes.copy")} />}
        <button
          className="p-1.5 rounded-md text-muted hover:text-error cursor-pointer"
          title={t("notes.delete")}
          onClick={handleDelete}
        >
          <Trash2 width={16} height={16} />
        </button>
      </div>

      <div className="flex flex-wrap items-center gap-2 ms-9">
        {(entry.ai_notes || generating || hasTranscript) && (
          <Chip
            icon={
              generating ? (
                <Loader2 size={12} className="animate-spin text-accent" />
              ) : (
                <Sparkles size={12} className="text-accent" />
              )
            }
            label={generating ? t("notes.writing") : t("shell.enhanced")}
            trailing={
              !generating && hasTranscript ? (
                <button
                  className="ms-0.5 cursor-pointer text-muted hover:text-text"
                  title={t("notes.regenerate")}
                  onClick={(e) => {
                    e.stopPropagation();
                    regenerate();
                  }}
                >
                  <RotateCcw size={11} />
                </button>
              ) : undefined
            }
          />
        )}
        <Chip
          icon={<Calendar size={12} />}
          label={formatDateTime(String(entry.timestamp), i18n.language)}
        />
        {hasTranscript && (
          <Chip
            icon={<Mic size={12} />}
            label={t("notes.transcriptChip")}
            active={showTranscript}
            onClick={() => setShowTranscript((prev) => !prev)}
          />
        )}
        <Chip icon={<HardDrive size={12} />} label={t("notes.onThisDevice")} />
      </div>

      {editing || !doc.trim() ? (
        <textarea
          ref={docRef}
          className="w-full ms-9 max-w-[68ch] bg-transparent resize-none outline-none border-none text-[15px] leading-7 text-text placeholder:text-faint placeholder:italic min-h-72 overflow-hidden"
          value={doc}
          placeholder={
            generating ? t("notes.writing") : t("notes.docPlaceholder")
          }
          onChange={(e) => setDraft(e.target.value)}
          onFocus={() => setEditing(true)}
          onBlur={() => setEditing(false)}
          spellCheck={false}
        />
      ) : (
        <div
          className="note-md ms-9 max-w-[68ch] cursor-text min-h-72"
          title={t("notes.docPlaceholder")}
          onClick={() => setEditing(true)}
        >
          <ReactMarkdown remarkPlugins={[remarkGfm]}>{doc}</ReactMarkdown>
        </div>
      )}

      {showTranscript && (
        <div className="ms-9 rounded-xl border border-border bg-card p-4 flex flex-col gap-3">
          <div className="flex items-center justify-between">
            <span className="text-xs font-semibold tracking-[0.05em] uppercase text-faint">
              {t("notes.transcriptChip")}
            </span>
            <CopyButton
              text={entry.transcription_text}
              label={t("notes.copy")}
            />
          </div>
          <AudioPlayerGroup>
            <AudioPlayer onLoadRequest={loadAudio} className="w-full" />
          </AudioPlayerGroup>
          <p className="text-sm leading-6 whitespace-pre-wrap select-text text-muted">
            {entry.transcription_text}
          </p>
        </div>
      )}
    </div>
  );
};

export const NotesSettings: React.FC<{
  openNote?: { id: number } | null;
}> = ({ openNote }) => {
  const { t, i18n } = useTranslation();
  const [entries, setEntries] = useState<HistoryEntry[]>([]);
  const [hasMore, setHasMore] = useState(false);
  const [loading, setLoading] = useState(true);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [statusById, setStatusById] = useState<Record<number, NotesStatus>>({});

  // Follow deep links from the Home view (a fresh object per request, so
  // opening the same note twice still re-selects it).
  useEffect(() => {
    if (openNote) setSelectedId(openNote.id);
  }, [openNote]);

  const loadPage = useCallback(async (cursor: number | null) => {
    const result = await commands.getMeetingEntries(cursor, PAGE_SIZE);
    if (result.status === "ok") {
      setEntries((prev) =>
        cursor === null
          ? result.data.entries
          : [...prev, ...result.data.entries],
      );
      setHasMore(result.data.has_more);
    } else {
      console.error("Failed to load notes:", result.error);
    }
    setLoading(false);
  }, []);

  useEffect(() => {
    loadPage(null);
  }, [loadPage]);

  // Typed history events keep the list and the open note fresh.
  useEffect(() => {
    const unlisten = events.historyUpdatePayload.listen((event) => {
      const payload = event.payload as HistoryUpdatePayload;
      if (payload.action === "added" && payload.entry.source === "meeting") {
        setEntries((prev) => [payload.entry, ...prev]);
      } else if (payload.action === "updated") {
        setEntries((prev) =>
          prev.map((e) => (e.id === payload.entry.id ? payload.entry : e)),
        );
      } else if (payload.action === "deleted") {
        setEntries((prev) => prev.filter((e) => e.id !== payload.id));
        setSelectedId((prev) => (prev === payload.id ? null : prev));
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // Notes may already be generating when this view mounts (a cold local
  // model can take a minute) — seed the status so the user sees "Writing
  // notes…" instead of an empty state that tempts a duplicate enhance.
  useEffect(() => {
    commands.getGeneratingNoteIds().then((result) => {
      if (result.status !== "ok") return;
      setStatusById((prev) => {
        const next = { ...prev };
        for (const id of result.data) next[id] = "generating";
        return next;
      });
    });
  }, []);

  useEffect(() => {
    const unlisten = listen<NotesStatusPayload>("notes-status", (event) => {
      setStatusById((prev) => ({
        ...prev,
        [event.payload.id]: event.payload.status,
      }));
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const selected = entries.find((e) => e.id === selectedId) ?? null;

  if (selected) {
    return (
      <div className="max-w-3xl w-full mx-auto">
        <NoteDetail
          key={selected.id}
          entry={selected}
          generating={statusById[selected.id] === "generating"}
          onBack={() => setSelectedId(null)}
        />
      </div>
    );
  }

  const snippet = (entry: HistoryEntry) => {
    const text =
      entry.user_notes?.trim() || entry.ai_notes || entry.transcription_text;
    const line = text.trim().split("\n")[0] ?? "";
    return line.length > 120 ? `${line.slice(0, 120)}…` : line;
  };

  return (
    <div className="max-w-3xl w-full mx-auto flex flex-col gap-4">
      <h2 className="font-serif text-2xl font-medium">{t("notes.title")}</h2>
      {loading ? (
        <p className="text-sm text-muted">{t("notes.loading")}</p>
      ) : entries.length === 0 ? (
        <div className="rounded-xl border border-border bg-card p-6 text-center flex flex-col gap-1">
          <p className="text-sm font-medium">{t("notes.empty")}</p>
          <p className="text-xs text-muted">{t("notes.emptyHint")}</p>
        </div>
      ) : (
        <div className="flex flex-col gap-0.5">
          {entries.map((entry) => (
            <div
              key={entry.id}
              className="px-3 py-[11px] rounded-[9px] cursor-pointer hover:bg-card2 transition-colors flex flex-col gap-0.5"
              onClick={() => setSelectedId(entry.id)}
            >
              <div className="flex items-center justify-between gap-2">
                <p className="font-serif text-[15px] leading-[18px] font-medium truncate">
                  {entry.title}
                </p>
                <p className="text-[11.5px] text-faint shrink-0">
                  {formatDateTime(String(entry.timestamp), i18n.language)}
                </p>
              </div>
              {statusById[entry.id] === "generating" ? (
                <p className="text-xs text-faint italic">
                  {t("notes.writing")}
                </p>
              ) : entry.ai_notes || entry.user_notes?.trim() ? (
                <p className="text-xs text-muted truncate">{snippet(entry)}</p>
              ) : (
                <p className="text-xs text-faint italic">
                  {entry.transcription_text.trim()
                    ? t("notes.noNotesYet")
                    : t("notes.noTranscript")}
                </p>
              )}
            </div>
          ))}
        </div>
      )}
      {hasMore && (
        <Button
          variant="secondary"
          size="sm"
          className="self-center"
          onClick={() => loadPage(entries[entries.length - 1]?.id ?? null)}
        >
          {t("notes.loadMore")}
        </Button>
      )}
    </div>
  );
};
