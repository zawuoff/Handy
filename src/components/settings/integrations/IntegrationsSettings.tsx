import React, { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { openUrl } from "@tauri-apps/plugin-opener";
import { X } from "lucide-react";
import { toast } from "sonner";
import { commands, type AppSettings } from "@/bindings";
import { SettingContainer, SettingsGroup } from "@/components/ui";
import { Button } from "../../ui/Button";
import { Input } from "../../ui/Input";
import { ApiKeyField } from "../PostProcessingSettingsApi/ApiKeyField";
import { useSettings } from "../../../hooks/useSettings";

type ConnectionState = "not_connected" | "pending" | "connected";

const toConnectionState = (status: string): ConnectionState =>
  status === "ACTIVE"
    ? "connected"
    : status === "not_connected"
      ? "not_connected"
      : "pending";

/** One connect-a-service row: status text + Connect button + OAuth polling. */
const ConnectRow: React.FC<{
  toolkit: string;
  accountKey: keyof AppSettings;
  title: string;
  description: string;
}> = ({ toolkit, accountKey, title, description }) => {
  const { t } = useTranslation();
  const { getSetting, refreshSettings } = useSettings();
  const apiKey = (getSetting("composio_api_key") as string) ?? "";
  const [connection, setConnection] = useState<ConnectionState>(
    ((getSetting(accountKey) as string) ?? "") !== ""
      ? "pending"
      : "not_connected",
  );
  const [connecting, setConnecting] = useState(false);
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    commands.getComposioConnectionStatus(toolkit).then((result) => {
      if (mounted.current && result.status === "ok") {
        setConnection(toConnectionState(result.data));
      }
    });
    return () => {
      mounted.current = false;
    };
  }, [toolkit]);

  const connect = async () => {
    setConnecting(true);
    try {
      const result = await commands.connectComposioToolkit(toolkit);
      if (result.status !== "ok") {
        toast.error(result.error);
        return;
      }
      await openUrl(result.data);
      setConnection("pending");
      // The backend just persisted the account id — pull it into the
      // frontend store so dependent buttons appear without a restart.
      await refreshSettings();
      // Poll while the user approves access in the browser (~2 min cap).
      for (let attempt = 0; attempt < 24 && mounted.current; attempt++) {
        await new Promise((resolve) => setTimeout(resolve, 5000));
        const status = await commands.getComposioConnectionStatus(toolkit);
        if (status.status === "ok" && status.data === "ACTIVE") {
          if (mounted.current) setConnection("connected");
          break;
        }
      }
    } finally {
      if (mounted.current) setConnecting(false);
    }
  };

  return (
    <SettingContainer
      title={title}
      description={description}
      descriptionMode="tooltip"
      layout="horizontal"
      grouped={true}
    >
      <div className="flex items-center gap-3">
        <span className="text-xs text-muted">
          {connection === "connected"
            ? t("settings.integrations.statusConnected")
            : connection === "pending"
              ? t("settings.integrations.statusPending")
              : t("settings.integrations.statusNotConnected")}
        </span>
        {connection !== "connected" && (
          <Button
            variant="secondary"
            size="sm"
            disabled={!apiKey || connecting}
            onClick={connect}
          >
            {t("settings.integrations.connect")}
          </Button>
        )}
      </div>
    </SettingContainer>
  );
};

/** Spoken-name → email contacts for the task key, plus the sign-off name. */
const GmailExtras: React.FC = () => {
  const { t } = useTranslation();
  const { getSetting, refreshSettings } = useSettings();
  const contacts =
    (getSetting("gmail_contacts") as Record<string, string>) ?? {};
  const signature = (getSetting("gmail_signature_name") as string) ?? "";
  const [newName, setNewName] = useState("");
  const [newEmail, setNewEmail] = useState("");
  const [sigDraft, setSigDraft] = useState<string | null>(null);

  const saveContacts = async (next: Record<string, string>) => {
    const result = await commands.changeGmailContactsSetting(next);
    if (result.status !== "ok") toast.error(result.error);
    await refreshSettings();
  };

  const addContact = async () => {
    const name = newName.trim();
    const email = newEmail.trim();
    if (!name || !email.includes("@")) return;
    await saveContacts({ ...contacts, [name]: email });
    setNewName("");
    setNewEmail("");
  };

  const removeContact = async (name: string) => {
    const next = { ...contacts };
    delete next[name];
    await saveContacts(next);
  };

  const saveSignature = async () => {
    if (sigDraft === null || sigDraft.trim() === signature) {
      setSigDraft(null);
      return;
    }
    const result = await commands.changeGmailSignatureNameSetting(sigDraft);
    if (result.status !== "ok") toast.error(result.error);
    await refreshSettings();
    setSigDraft(null);
  };

  return (
    <>
      <SettingContainer
        title={t("settings.integrations.contacts.title")}
        description={t("settings.integrations.contacts.description")}
        descriptionMode="tooltip"
        layout="stacked"
        grouped={true}
      >
        <div className="flex flex-col gap-2 w-full">
          {Object.entries(contacts)
            .sort(([a], [b]) => a.localeCompare(b))
            .map(([name, email]) => (
              <div key={name} className="flex items-center gap-2">
                <span className="text-sm min-w-[120px]">{name}</span>
                <span className="text-xs text-muted flex-1 truncate">
                  {email}
                </span>
                <button
                  className="p-1 text-muted hover:text-error cursor-pointer"
                  title={t("notes.delete")}
                  onClick={() => removeContact(name)}
                >
                  <X size={14} />
                </button>
              </div>
            ))}
          <div className="flex items-center gap-2">
            <Input
              variant="compact"
              placeholder={t("settings.integrations.contacts.namePlaceholder")}
              value={newName}
              onChange={(e) => setNewName(e.target.value)}
              className="max-w-[160px]"
            />
            <Input
              variant="compact"
              placeholder={t("settings.integrations.contacts.emailPlaceholder")}
              value={newEmail}
              onChange={(e) => setNewEmail(e.target.value)}
              className="flex-1"
              onKeyDown={(e) => {
                if (e.key === "Enter") addContact();
              }}
            />
            <Button
              variant="secondary"
              size="sm"
              disabled={!newName.trim() || !newEmail.includes("@")}
              onClick={addContact}
            >
              {t("settings.integrations.contacts.add")}
            </Button>
          </div>
        </div>
      </SettingContainer>
      <SettingContainer
        title={t("settings.integrations.signature.title")}
        description={t("settings.integrations.signature.description")}
        descriptionMode="tooltip"
        layout="horizontal"
        grouped={true}
      >
        <Input
          variant="compact"
          placeholder={t("settings.integrations.signature.placeholder")}
          value={sigDraft ?? signature}
          onChange={(e) => setSigDraft(e.target.value)}
          onBlur={saveSignature}
          className="min-w-[200px]"
        />
      </SettingContainer>
    </>
  );
};

/**
 * Settings page for third-party integrations via Composio. Each service is
 * one ConnectRow; adding a toolkit later is one more row.
 */
export const IntegrationsSettings: React.FC = () => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();
  const apiKey = (getSetting("composio_api_key") as string) ?? "";

  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      <SettingsGroup title={t("settings.integrations.composio.title")}>
        <SettingContainer
          title={t("settings.integrations.apiKey.title")}
          description={t("settings.integrations.apiKey.description")}
          descriptionMode="tooltip"
          layout="horizontal"
          grouped={true}
        >
          <div className="flex items-center gap-2">
            <ApiKeyField
              value={apiKey}
              onBlur={(value) => {
                if (value !== apiKey) {
                  updateSetting("composio_api_key", value);
                }
              }}
              placeholder={t("settings.integrations.apiKey.placeholder")}
              disabled={isUpdating("composio_api_key")}
              className="min-w-[320px]"
            />
          </div>
        </SettingContainer>
        <ConnectRow
          toolkit="googledocs"
          accountKey="composio_gdocs_account_id"
          title={t("settings.integrations.gdocs.title")}
          description={t("settings.integrations.gdocs.description")}
        />
        <ConnectRow
          toolkit="gmail"
          accountKey="composio_gmail_account_id"
          title={t("settings.integrations.gmail.title")}
          description={t("settings.integrations.gmail.description")}
        />
        <GmailExtras />
      </SettingsGroup>
    </div>
  );
};
