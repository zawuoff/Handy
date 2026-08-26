import React, { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { CalendarPlus, Check, Plus, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { commands, type Todo } from "@/bindings";

const TodoRow: React.FC<{ todo: Todo }> = ({ todo }) => {
  const { t } = useTranslation();
  const [scheduling, setScheduling] = useState(false);
  const [when, setWhen] = useState("");

  const toggle = async () => {
    await commands.setTodoDone(todo.id, !todo.done);
  };

  const remove = async () => {
    await commands.deleteTodo(todo.id);
  };

  const schedule = async () => {
    const value = when.trim();
    if (!value) return;
    const result = await commands.todoToEvent(todo.id, value, null);
    if (result.status === "ok") {
      toast.success(t("todos.eventCreated", { title: todo.title }));
      setScheduling(false);
      setWhen("");
    } else {
      toast.error(t("todos.eventFailed"), { description: result.error });
    }
  };

  return (
    <div className="px-3 py-2.5 flex flex-col gap-2">
      <div className="flex items-center gap-3">
        <button
          className={`w-[18px] h-[18px] shrink-0 rounded-[5px] border flex items-center justify-center cursor-pointer transition-colors ${
            todo.done
              ? "bg-accent border-accent text-background"
              : "border-border-strong hover:border-accent"
          }`}
          onClick={toggle}
          aria-checked={todo.done}
          role="checkbox"
        >
          {todo.done && <Check size={12} />}
        </button>
        <span
          className={`flex-1 text-sm min-w-0 truncate ${
            todo.done ? "line-through text-faint" : "text-text"
          }`}
        >
          {todo.title}
        </span>
        {todo.source_entry_id != null && (
          <span className="shrink-0 rounded-md bg-card2 px-2 py-0.5 text-[10.5px] font-medium text-muted">
            {t("todos.fromMeeting")}
          </span>
        )}
        {!todo.done && (
          <button
            className="p-1.5 rounded-md text-muted hover:text-accent cursor-pointer shrink-0"
            title={t("todos.makeEvent")}
            onClick={() => setScheduling((prev) => !prev)}
          >
            <CalendarPlus size={15} />
          </button>
        )}
        <button
          className="p-1.5 rounded-md text-muted hover:text-error cursor-pointer shrink-0"
          title={t("common.delete")}
          onClick={remove}
        >
          <Trash2 size={14} />
        </button>
      </div>
      {scheduling && (
        <div className="flex items-center gap-2 ms-8">
          <input
            className="flex-1 bg-card border border-border rounded-lg px-3 py-1.5 text-sm outline-none focus:border-accent min-w-0"
            value={when}
            placeholder={t("todos.whenPlaceholder")}
            autoFocus
            onChange={(e) => setWhen(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") schedule();
              if (e.key === "Escape") setScheduling(false);
            }}
          />
          <button
            className="rounded-lg bg-text text-background px-3 py-1.5 text-xs font-semibold cursor-pointer hover:opacity-90"
            onClick={schedule}
          >
            {t("todos.schedule")}
          </button>
        </div>
      )}
    </div>
  );
};

export const TodosView: React.FC = () => {
  const { t } = useTranslation();
  const [todos, setTodos] = useState<Todo[]>([]);
  const [newTitle, setNewTitle] = useState("");

  const refresh = useCallback(async () => {
    const result = await commands.getTodos();
    if (result.status === "ok") setTodos(result.data);
  }, []);

  useEffect(() => {
    refresh();
    const unlisten = listen("todos-updated", () => refresh());
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [refresh]);

  const add = async () => {
    const title = newTitle.trim();
    if (!title) return;
    setNewTitle("");
    await commands.addTodo(title);
  };

  const open = todos.filter((todo) => !todo.done);
  const done = todos.filter((todo) => todo.done);

  return (
    <div className="max-w-3xl w-full mx-auto flex flex-col gap-4">
      <h2 className="font-serif text-2xl font-medium">{t("todos.title")}</h2>

      <div className="flex items-center gap-2">
        <input
          className="flex-1 bg-card border border-border rounded-lg px-3 py-2 text-sm outline-none focus:border-accent min-w-0"
          value={newTitle}
          placeholder={t("todos.placeholder")}
          onChange={(e) => setNewTitle(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") add();
          }}
        />
        <button
          className="p-2 rounded-lg bg-text text-background cursor-pointer hover:opacity-90 shrink-0"
          title={t("todos.add")}
          onClick={add}
        >
          <Plus size={16} />
        </button>
      </div>

      {todos.length === 0 ? (
        <div className="rounded-xl border border-border bg-card p-6 text-center flex flex-col gap-1">
          <p className="text-sm font-medium">{t("todos.empty")}</p>
          <p className="text-xs text-faint">{t("todos.emptyHint")}</p>
        </div>
      ) : (
        <div className="rounded-xl border border-border bg-card divide-y divide-border">
          {open.map((todo) => (
            <TodoRow key={todo.id} todo={todo} />
          ))}
          {done.map((todo) => (
            <TodoRow key={todo.id} todo={todo} />
          ))}
        </div>
      )}
    </div>
  );
};
