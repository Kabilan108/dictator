import {
  createContext,
  useContext,
  useState,
  useEffect,
  ReactNode,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { DictatorConfig, ModelInfo, Result } from "@/types";
import { useTheme } from "@/contexts/ThemeContext";
import { Log } from "@/lib/utils";
import { ThemeName } from "@/lib/themes";

interface SettingsContextType {
  errorMessage: string;
  isEditing: boolean;
  isSubmitting: boolean;
  models: ModelInfo[];
  settings: DictatorConfig;
  supportsModels: boolean;
  testSuccess: boolean | null;
  cancelEdit: () => void;
  handleChange: (field: keyof DictatorConfig, value: string) => void;
  loadSettings: () => Promise<void>;
  saveSettings: () => Promise<void>;
  testConnection: () => Promise<void>;
  toggleEditMode: () => void;
}

const SettingsContext = createContext<SettingsContextType | undefined>(undefined);

export const SettingsProvider = ({ children }: { children: ReactNode }) => {
  const { themeName: currentTheme, changeTheme } = useTheme();

  // form state
  const [settings, setSettings] = useState<DictatorConfig>({
    apiUrl: "",
    apiKey: "",
    defaultModel: "",
    theme: currentTheme,
  });
  const [initialSettings, setInitialSettings] = useState<Partial<DictatorConfig>>({});

  const [models, setModels] = useState<ModelInfo[]>([]);
  const [supportsModels, setSupportsModels] = useState<boolean>(false);

  const [isEditing, setIsEditing] = useState(false);
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [errorMessage, setErrorMessage] = useState("");
  const [testSuccess, setTestSuccess] = useState<boolean | null>(null);

  const loadSettings = async () => {
    try {
      Log.d("Loading settings...")
      const cfg: DictatorConfig = await invoke("get_settings");
      const support: boolean = await invoke("supports_models_endpoint")
      setSupportsModels(support)

      const merged = {
        apiUrl: cfg.apiUrl,
        apiKey: cfg.apiKey,
        defaultModel: cfg.defaultModel,
        theme: cfg.theme || currentTheme
      }
      setSettings(merged)
      setInitialSettings(merged)
      if (cfg.theme && cfg.theme !== currentTheme) {
        changeTheme(cfg.theme as ThemeName)
      }
      if (support) {
        const models: ModelInfo[] = await invoke("list_available_models");
        setModels(models);
      } else {
        setModels([]);
      }
    } catch (e) {
      const msg = `Failed to load settings: ${e instanceof Error ? e.message : String(e)}`
      Log.e(msg);
      setErrorMessage(msg);
    }
  }

  const saveSettings = async () => {
    setIsSubmitting(true);
    setErrorMessage("");
    Log.d("Saving settings...", settings);
    try {
      const res: Result = await invoke("save_settings", { settings });
      if (!res.success) {
        throw new Error(res.error || "Unkown error");
      }
      setInitialSettings(settings);
      setIsEditing(false);
      // re-check endpoint/support
      const support: boolean = await invoke("supports_models_endpoint");
      setSupportsModels(support);
      if (support) {
        const models: ModelInfo[] = await invoke("list_available_models");
        setModels(models);
      } else {
        setModels([]);
      }
    } catch (e) {
      const msg = `Save failed ${e instanceof Error ? e.message : String(e)}`
      Log.e(msg);
      setErrorMessage(msg)
    } finally {
      setIsSubmitting(false);
    }
  }

  const testConnection = async () => {
    setIsSubmitting(true);
    setErrorMessage("");
    setTestSuccess(null);
    Log.d("Testing connection…", settings);
    try {
      // temporarily save → test → restore
      await invoke("save_settings", { settings });
      const support: boolean = await invoke("supports_models_endpoint");
      setSupportsModels(support);
      if (support) {
        await invoke("list_available_models");
        setTestSuccess(true);
      } else {
        setTestSuccess(true);
        setErrorMessage("API reachable but models endpoint not found");
      }
    } catch (e) {
      const msg = `Test failed: ${e instanceof Error ? e.message : String(e)}`;
      Log.e(msg);
      setTestSuccess(false);
      setErrorMessage(msg);
    } finally {
      // restore original
      await invoke("save_settings", { settings: initialSettings });
      setIsSubmitting(false);
    }
  };

  const toggleEditMode = () => {
    setErrorMessage("");
    setTestSuccess(null);
    setIsEditing((v) => !v);
  };

  const cancelEdit = () => {
    setSettings(initialSettings as DictatorConfig);
    setErrorMessage("");
    setTestSuccess(null);
    setIsEditing(false);
    if (initialSettings.theme !== currentTheme) {
      changeTheme(initialSettings.theme as any);
    }
  };

  const handleChange = (field: keyof DictatorConfig, value: string) => {
    setSettings((prev) => ({ ...prev, [field]: value }));
    setErrorMessage("");
    setTestSuccess(null);
  };

  useEffect(() => {
    loadSettings();
  }, []);

  return (
    <SettingsContext.Provider
      value={{
        settings,
        models,
        supportsModels,
        isEditing,
        isSubmitting,
        errorMessage,
        testSuccess,
        loadSettings,
        saveSettings,
        testConnection,
        toggleEditMode,
        cancelEdit,
        handleChange,
      }}
    >
      {children}
    </SettingsContext.Provider>
  );
}

export const useSettings = () => {
  const c = useContext(SettingsContext);
  if (!c) throw new Error("useSettings must be inside SettingsProivder");
  return c;
}
