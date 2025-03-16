import { useState, useEffect } from "react";

import { themes, ThemeName } from '@/lib/themes';
import { useTheme } from "@/lib/ThemeContext";
import { app } from "@wailsjs/go/models";
import { GetSettings, ListAvailableModels, SaveSettings } from "@wailsjs/go/main/App";
import { Log } from "@/lib/utils";

interface SettingsForm {
  apiUrl: string;
  apiKey: string;
  defaultModel: string;
  theme: string;
  isEditing: boolean;
  isSubmitting: boolean;
  errorMessage: string;
  testSuccess: boolean | null;
}

interface FormProps {
  settings: SettingsForm,
  handleChange: (field: keyof SettingsForm, value: string) => void,
}

interface APISettingsProps extends FormProps {
  availableModels: app.ModelInfo[],
  testConnection: () => void,
}

interface FormButtonProps extends FormProps {
  cancelEdit: () => void,
  saveSettings: () => void,
  toggleEditMode: () => void,
}

const ThemeSelection = ({ settings, handleChange }: FormProps) => {
  const { colors, changeTheme } = useTheme();

  const onChangeTheme = (theme: string) => {
    changeTheme(theme as ThemeName)
    handleChange("theme", theme)
  }

  return (
    <div
      className="flex justify-between items-center text-sm mb-2"
    >
      <label>Theme</label>
      <select
        value={settings.theme}
        onChange={(e) => onChangeTheme(e.target.value)}
        disabled={!settings.isEditing}
        className="border rounded px-1"
        style={{
          background: !settings.isEditing ? colors.mantle : colors.surface0,
          borderColor: !settings.isEditing ? colors.mantle : colors.surface1,
          color: colors.text,
          opacity: !settings.isEditing ? 0.7 : 1
        }}
      >
        {Object.keys(themes).map((name) => (
          <option
            key={name}
            value={name}
            style={{
              backgroundColor: colors.surface0,
              color: colors.text
            }}
          >
            {themes[name as ThemeName].name}
          </option>
        ))}
      </select>
    </div>
  )
}

const APISettings = ({
  availableModels,
  settings,
  handleChange,
  testConnection,
}: APISettingsProps) => {
  const { colors } = useTheme();

  return (
    <div className="mb-4">
      <h4
        className="text-sm mb-3 pb-1 border-b"
        style={{ borderColor: colors.surface1 }}
      >
        Whisper API Settings
      </h4>

      {/* API URL */}
      <div className="mb-2">
        <div className="flex justify-between items-center">
          <label className="text-sm">API URL</label>
        </div>
        <input
          type="text"
          value={settings.apiUrl}
          onChange={(e) => handleChange('apiUrl', e.target.value)}
          disabled={!settings.isEditing}
          className="w-full px-3 py-1 rounded mt-1"
          style={{
            backgroundColor: colors.surface0,
            color: colors.text,
            borderColor: colors.surface1
          }}
        />
      </div>

      {/* API Key */}
      <div className="mb-2">
        <div className="flex justify-between items-center">
          <label className="text-sm">API Key</label>
        </div>
        <input
          type="password"
          value={settings.apiKey}
          onChange={(e) => handleChange('apiKey', e.target.value)}
          disabled={!settings.isEditing}
          className="w-full px-3 py-1 rounded mt-1"
          style={{
            backgroundColor: colors.surface0,
            color: colors.text,
            borderColor: colors.surface1
          }}
        />
      </div>

      {/* Model */}
      <div className="mb-3">
        <div className="flex justify-between items-center">
          <label className="text-sm">Model</label>
        </div>
        {availableModels.length > 0 && settings.isEditing ? (
          <select
            value={settings.defaultModel}
            onChange={(e) => handleChange('defaultModel', e.target.value)}
            className="w-full px-3 py-1 rounded mt-1"
            style={{
              backgroundColor: colors.surface0,
              color: colors.text,
              borderColor: colors.surface1
            }}
          >
            <option value="">Select a model</option>
            {availableModels.map((model) => (
              <option
                key={model.id}
                value={model.id}
                style={{
                  backgroundColor: colors.surface0,
                  color: colors.text
                }}
              >
                {model.id}
              </option>
            ))}
          </select>
        ) : (
          <input
            type="text"
            value={settings.defaultModel}
            onChange={(e) => handleChange('defaultModel', e.target.value)}
            disabled={!settings.isEditing}
            className="w-full px-3 py-1 rounded mt-1"
            style={{
              backgroundColor: colors.surface0,
              color: colors.text,
              borderColor: colors.surface1
            }}
          />
        )}
      </div>

      {/* Test Connection Button */}
      {settings.isEditing && (
        <button
          onClick={testConnection}
          disabled={settings.isSubmitting}
          className="w-[50%] mx-auto mb-3 py-1 px-1 flex rounded-md justify-center items-center rounded"
          style={{
            backgroundColor: colors.surface1,
            color: colors.text
          }}
        >
          {settings.isSubmitting ? (
            <div className="animate-spin h-4 w-4 border-2 rounded-full border-b-transparent mr-2"
              style={{ borderColor: colors.text, borderBottomColor: 'transparent' }} />
          ) : null}
          Test Connection
        </button>
      )}

      {/* Success Message */}
      {settings.testSuccess === true && !settings.errorMessage && (
        <div
          className="text-sm px-3 py-2 rounded mb-3"
          style={{
            backgroundColor: colors.green + '20',
            color: colors.green
          }}
        >
          Connection successful!
        </div>
      )}

    </div>
  )
}

const FormButtons = ({
  settings,
  cancelEdit,
  saveSettings,
  toggleEditMode,
}: FormButtonProps) => {
  const { colors } = useTheme();

  return (
    <div className="flex justify-end gap-2 mb-2 sticky bottom-0 bg-inherit pt-2 px-4">
      {!settings.isEditing ? (
        <button
          onClick={toggleEditMode}
          className="py-1 px-3 rounded"
          style={{
            backgroundColor: colors.surface0,
            color: colors.text
          }}
        >
          Edit
        </button>
      ) : (
        <>
          <button
            onClick={cancelEdit}
            className="py-1 px-3 rounded"
            style={{
              backgroundColor: 'transparent',
              color: colors.overlay
            }}
          >
            Cancel
          </button>
          <button
            onClick={saveSettings}
            disabled={settings.isSubmitting}
            className="py-1 px-5 rounded"
            style={{
              backgroundColor: colors.accent,
              color: colors.base
            }}
          >
            {settings.isSubmitting ? 'Saving...' : 'Save'}
          </button>
        </>
      )}
    </div>
  )
}

const SettingsPanel = () => {
  const { colors } = useTheme();
  const [settings, setSettings] = useState<SettingsForm>({
    apiUrl: "",
    apiKey: "",
    defaultModel: "",
    theme: "",
    isEditing: false,
    isSubmitting: false,
    errorMessage: "",
    testSuccess: null,
  });
  const [availableModels, setAvailableModels] = useState<app.ModelInfo[]>([]);

  // Fetch current settings when the component mounts
  useEffect(() => {
    fetchSettings();
  }, []);

  const fetchSettings = async () => {
    try {
      const response = await GetSettings();
      setSettings(prev => ({
        ...prev,
        apiUrl: response.apiUrl,
        apiKey: response.apiKey,
        defaultModel: response.defaultModel
      }));

      // If models endpoint is supported, fetch available models
      if (response.supportsModels) {
        fetchModels();
      }
    } catch (error) {
      setSettings(prev => ({
        ...prev,
        errorMessage: "Failed to load settings"
      }));
      Log.e("Failed to fetch settings:", error);
    }
  };

  const fetchModels = async () => {
    try {
      const models = await ListAvailableModels();
      setAvailableModels(models);
    } catch (error) {
      Log.e("Failed to fetch models:", error);
    }
  };

  const handleChange = (field: keyof SettingsForm, value: string) => {
    setSettings(prev => ({
      ...prev,
      [field]: value,
      errorMessage: "", // Clear previous errors
      testSuccess: null, // Reset test status
    }));
  };

  const toggleEditMode = () => {
    setSettings(prev => ({
      ...prev,
      isEditing: !prev.isEditing,
      errorMessage: "",
      testSuccess: null,
    }));
  };

  const testConnection = async () => {
    setSettings(prev => ({
      ...prev,
      isSubmitting: true,
      errorMessage: "",
      testSuccess: null
    }));

    try {
      // Create a temporary settings object for testing
      const testSettings = {
        apiUrl: settings.apiUrl,
        apiKey: settings.apiKey,
        defaultModel: settings.defaultModel,
        supportsModels: false, // Will be determined by the test
        theme: settings.theme,
      };

      // Save settings temporarily
      const result = await SaveSettings(testSettings);

      if (!result.success) {
        throw new Error(result.error || "Unknown error");
      }

      // Try to list models as a connection test
      try {
        await ListAvailableModels();
        setSettings(prev => ({
          ...prev,
          isSubmitting: false,
          testSuccess: true
        }));
      } catch (error) {
        // If ListAvailableModels fails, the connection might still work but models aren't supported
        setSettings(prev => ({
          ...prev,
          isSubmitting: false,
          testSuccess: true,
          errorMessage: "Connected, but models endpoint not supported"
        }));
      }
    } catch (error) {
      setSettings(prev => ({
        ...prev,
        isSubmitting: false,
        testSuccess: false,
        errorMessage: error instanceof Error ? error.message : "Connection test failed"
      }));
      Log.e("Test connection failed:", error);
    }
  };

  const saveSettings = async () => {
    setSettings(prev => ({
      ...prev,
      isSubmitting: true,
      errorMessage: ""
    }));

    try {
      const newSettings = {
        apiUrl: settings.apiUrl,
        apiKey: settings.apiKey,
        defaultModel: settings.defaultModel,
        supportsModels: false, // Will be updated by the backend
        theme: settings.theme
      };

      const result = await SaveSettings(newSettings);

      if (!result.success) {
        throw new Error(result.error || "Failed to save settings");
      }

      // Refresh settings and exit edit mode
      await fetchSettings();
      setSettings(prev => ({
        ...prev,
        isSubmitting: false,
        isEditing: false
      }));
    } catch (error) {
      setSettings(prev => ({
        ...prev,
        isSubmitting: false,
        errorMessage: error instanceof Error ? error.message : "Failed to save settings"
      }));
      Log.e("Save settings failed:", error);
    }
  };

  const cancelEdit = () => {
    // Reset form to current settings and exit edit mode
    fetchSettings();
    setSettings(prev => ({
      ...prev,
      isEditing: false,
      errorMessage: "",
      testSuccess: null
    }));
  };

  return (
    <div
      className="flex flex-col h-full"
      style={{ maxHeight: "calc(400px - 60px)" }} // Account for header height
    >
      <div className="flex items-center justify-center w-full mb-4">
        <h3 className="font-medium">Settings</h3>
      </div>

      <div
        className="flex-1 overflow-y-auto px-4 space-y-4"
        style={{
          scrollbarWidth: 'thin',
          scrollbarColor: `${colors.surface2} ${colors.surface0}`
        }}
      >
        {/* Theme Selection */}
        <ThemeSelection settings={settings} handleChange={handleChange} />

        {/* API Settings */}
        <APISettings
          availableModels={availableModels}
          settings={settings}
          handleChange={handleChange}
          testConnection={testConnection}
        />
      </div>

      <FormButtons
        settings={settings}
        cancelEdit={cancelEdit}
        handleChange={handleChange}
        saveSettings={saveSettings}
        toggleEditMode={toggleEditMode}
      />
    </div>
  );
};

export default SettingsPanel;
