import React, { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { ArrowLeft, Loader2, RotateCcw, Sparkles, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import {
  commands,
  type AskSession,
  type SearchHit,
  type PostProcessProvider,
} from "@/bindings";
import { useSettings } from "@/hooks/useSettings";
import { formatDateTime } from "@/utils/dateFormat";

const PROVIDER_STORAGE_KEY = "noted.askProvider";

export const storedAskProvider = (): string | null =>
  localStorage.getItem(PROVIDER_STORAGE_KEY);

/**
 * One "ask your notes" session: the query as the page, an answer written by
 * the chosen provider, and the matching meetings underneath as sources.
 */
export const AskView: React.FC<{
  sessionId: number;
  onBack: () => void;
  onOpenNote: (id: number) => void;
}> = ({ sessionId, onBack, onOpenNote }) => {
  const { t, i18n } = useTranslation();
  const { getSetting } = useSettings();
  const [session, setSession] = useState<AskSession | null>(null);
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [thinking, setThinking] = useState(false);
  const [failed, setFailed] = useState(false);
  const startedRef = useRef(false);

  const providers = (
    (getSetting("post_process_providers") as PostProcessProvider[]) ?? []
  ).filter((provider) => provider.id !== "apple_intelligence");
  const models =
    (getSetting("post_process_models") as Record<string, string>) ?? {};
  const usableProviders = providers.filter((provider) =>
    (models[provider.id] ?? "").trim(),
  );
  const activeProviderId =
    (getSetting("post_process_provider_id") as string) ?? "custom";
  const [providerId, setProviderId] = useState<string>(
    () => storedAskProvider() ?? activeProviderId,
  );

  const refreshSession = useCallback(async () => {
    const result = await commands.listAskSessions();
    if (result.status !== "ok") return null;
    const found = result.data.find((entry) => entry.id === sessionId) ?? null;
    setSession(found);
    return found;
  }, [sessionId]);

  // Load the session and its matching meetings; kick off the answer once if
  // it hasn't been written yet.
  useEffect(() => {
    startedRef.current = false;
    setFailed(false);
    setThinking(false);
    setHits([]);
    (async () => {
      const found = await refreshSession();
      if (!found) return;
      const search = await commands.searchNotes(found.query, 20);
      if (search.status === "ok") setHits(search.data);
      if (
        !found.answer &&
        !startedRef.current &&
        search.status === "ok" &&
        search.data.length > 0
      ) {
        startedRef.current = true;
        setThinking(true);
        commands.answerAskSession(found.id, providerId);
      }
    })();
    // providerId intentionally omitted: switching providers only affects the
    // explicit regenerate action, not the initial auto-answer.
  }, [sessionId, refreshSession]);

  useEffect(() => {
    const unlisten = listen<{ id: number; status: string }>(
      "ask-answer",
      (event) => {
        if (event.payload.id !== sessionId) return;
        if (event.payload.status === "thinking") {
          setThinking(true);
          setFailed(false);
        } else if (event.payload.status === "done") {
          setThinking(false);
          refreshSession();
        } else {
          setThinking(false);
          setFailed(true);
        }
      },
    );
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [sessionId, refreshSession]);

  const regenerate = () => {
    setFailed(false);
    setThinking(true);
    commands.answerAskSession(sessionId, providerId);
  };

  const selectProvider = (id: string) => {
    setProviderId(id);
    localStorage.setItem(PROVIDER_STORAGE_KEY, id);
  };

  const handleDelete = async () => {
    await commands.deleteAskSession(sessionId);
    onBack();
  };

  if (!session) return null;

  return (
    <div className="max-w-3xl w-full mx-auto flex flex-col gap-5">
      <div className="flex items-center gap-2">
        <button
          className="p-1.5 rounded-md text-muted hover:text-text cursor-pointer"
          title={t("notes.back")}
          onClick={onBack}
        >
          <ArrowLeft width={18} height={18} />
        </button>
        <h1 className="flex-1 font-serif text-[26px] leading-tight font-medium min-w-0">
          {session.query}
        </h1>
        <button
          className="p-1.5 rounded-md text-muted hover:text-error cursor-pointer"
          title={t("common.delete")}
          onClick={handleDelete}
        >
          <Trash2 size={16} />
        </button>
      </div>

      <div className="rounded-xl border border-border bg-card p-4 flex flex-col gap-3">
        <div className="flex items-center justify-between gap-2">
          <span className="text-xs font-semibold tracking-[0.05em] uppercase text-faint flex items-center gap-1.5">
            <Sparkles size={12} className="text-accent" />
            {t("ask.answer")}
          </span>
          <div className="flex items-center gap-1.5">
            {usableProviders.length > 1 && (
              <select
                className="bg-card2 border border-border rounded-md px-2 py-1 text-xs text-muted outline-none cursor-pointer"
                value={providerId}
                onChange={(e) => selectProvider(e.target.value)}
                title={t("ask.providerHint")}
              >
                {usableProviders.map((provider) => (
                  <option key={provider.id} value={provider.id}>
                    {provider.label}
                  </option>
                ))}
              </select>
            )}
            <button
              className="p-1.5 rounded-md text-muted hover:text-text cursor-pointer disabled:text-text/20 disabled:cursor-not-allowed"
              title={t("ask.regenerate")}
              disabled={thinking || hits.length === 0}
              onClick={regenerate}
            >
              <RotateCcw size={14} className={thinking ? "animate-spin" : ""} />
            </button>
          </div>
        </div>
        {thinking ? (
          <p className="text-sm text-faint italic flex items-center gap-2">
            <Loader2 size={14} className="animate-spin" />
            {t("ask.thinking")}
          </p>
        ) : failed ? (
          <p className="text-sm text-error">{t("ask.failed")}</p>
        ) : session.answer ? (
          <div className="note-md">
            <ReactMarkdown remarkPlugins={[remarkGfm]}>
              {session.answer}
            </ReactMarkdown>
          </div>
        ) : hits.length === 0 ? (
          <p className="text-sm text-faint italic">{t("ask.noMatches")}</p>
        ) : null}
      </div>

      {hits.length > 0 && (
        <div className="flex flex-col gap-1">
          <span className="text-xs font-semibold tracking-[0.05em] uppercase text-faint pb-1">
            {t("ask.sources")}
          </span>
          {hits.map((hit) => (
            <button
              key={hit.entry_id}
              className="text-start px-3 py-[11px] rounded-[9px] cursor-pointer hover:bg-card2 transition-colors flex flex-col gap-0.5"
              onClick={() => onOpenNote(hit.entry_id)}
            >
              <span className="flex items-center justify-between gap-2">
                <span className="font-serif text-[15px] leading-[18px] font-medium truncate">
                  {hit.title}
                </span>
                <span className="text-[11.5px] text-faint shrink-0">
                  {formatDateTime(String(hit.timestamp), i18n.language)}
                </span>
              </span>
              {hit.snippet && (
                <span className="text-xs text-muted line-clamp-2">
                  {hit.snippet}
                </span>
              )}
            </button>
          ))}
        </div>
      )}
    </div>
  );
};
