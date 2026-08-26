import React, { useEffect } from "react";
import { EditorContent, useEditor, type Editor } from "@tiptap/react";
import StarterKit from "@tiptap/starter-kit";
import { Markdown } from "tiptap-markdown";
import {
  Bold,
  Heading2,
  Italic,
  List,
  ListOrdered,
  Strikethrough,
} from "lucide-react";
import { useTranslation } from "react-i18next";

// tiptap-markdown ships TipTap v2 type augmentations, so its storage entry
// is invisible to v3's Storage type — narrow it here.
const getMarkdown = (editor: Editor): string =>
  (
    editor.storage as unknown as {
      markdown: { getMarkdown: () => string };
    }
  ).markdown.getMarkdown();

const ToolbarButton: React.FC<{
  icon: React.ReactNode;
  label: string;
  active: boolean;
  onClick: () => void;
}> = ({ icon, label, active, onClick }) => (
  <button
    className={`p-1.5 rounded-md cursor-pointer transition-colors ${
      active ? "bg-card2 text-text" : "text-muted hover:text-text"
    }`}
    title={label}
    onMouseDown={(e) => {
      // Keep the editor selection alive while clicking the toolbar.
      e.preventDefault();
      onClick();
    }}
  >
    {icon}
  </button>
);

const Toolbar: React.FC<{ editor: Editor }> = ({ editor }) => {
  const { t } = useTranslation();
  return (
    <div className="flex items-center gap-0.5 rounded-lg border border-border bg-card px-1 py-0.5 w-fit">
      <ToolbarButton
        icon={<Bold size={14} />}
        label={t("notes.toolbarBold")}
        active={editor.isActive("bold")}
        onClick={() => editor.chain().focus().toggleBold().run()}
      />
      <ToolbarButton
        icon={<Italic size={14} />}
        label={t("notes.toolbarItalic")}
        active={editor.isActive("italic")}
        onClick={() => editor.chain().focus().toggleItalic().run()}
      />
      <ToolbarButton
        icon={<Strikethrough size={14} />}
        label={t("notes.toolbarStrike")}
        active={editor.isActive("strike")}
        onClick={() => editor.chain().focus().toggleStrike().run()}
      />
      <span className="w-px h-4 bg-border mx-0.5" />
      <ToolbarButton
        icon={<Heading2 size={14} />}
        label={t("notes.toolbarHeading")}
        active={editor.isActive("heading", { level: 2 })}
        onClick={() => editor.chain().focus().toggleHeading({ level: 2 }).run()}
      />
      <ToolbarButton
        icon={<List size={14} />}
        label={t("notes.toolbarBullets")}
        active={editor.isActive("bulletList")}
        onClick={() => editor.chain().focus().toggleBulletList().run()}
      />
      <ToolbarButton
        icon={<ListOrdered size={14} />}
        label={t("notes.toolbarNumbered")}
        active={editor.isActive("orderedList")}
        onClick={() => editor.chain().focus().toggleOrderedList().run()}
      />
    </div>
  );
};

/**
 * WYSIWYG note editor (TipTap) that reads and writes Markdown, so the stored
 * format stays exactly what the note generation produces and the MCP serves.
 * Bold/italic render as they are typed; ctrl+B / ctrl+I / `- ` / `## ` all
 * work as expected.
 */
export const RichNoteEditor: React.FC<{
  /** Markdown source of truth. */
  content: string;
  onChangeMarkdown: (markdown: string) => void;
  placeholder: string;
}> = ({ content, onChangeMarkdown, placeholder }) => {
  const editor = useEditor({
    extensions: [StarterKit, Markdown],
    content,
    shouldRerenderOnTransaction: true,
    editorProps: {
      attributes: {
        class: "note-md outline-none min-h-72 pb-24",
      },
    },
    onUpdate: ({ editor }) => {
      onChangeMarkdown(getMarkdown(editor));
    },
  });

  // Follow external content changes (e.g. freshly generated notes arriving)
  // without clobbering what the user is actively typing.
  useEffect(() => {
    if (!editor || editor.isFocused) return;
    const current = getMarkdown(editor);
    if (current.trim() !== content.trim()) {
      editor.commands.setContent(content);
    }
  }, [editor, content]);

  if (!editor) return null;

  return (
    <div className="flex flex-col gap-3 w-full relative">
      <Toolbar editor={editor} />
      {editor.isEmpty && (
        <span className="absolute top-12 text-[15px] text-faint italic pointer-events-none">
          {placeholder}
        </span>
      )}
      <EditorContent editor={editor} className="cursor-text" />
    </div>
  );
};
