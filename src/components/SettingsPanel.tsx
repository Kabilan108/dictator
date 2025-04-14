import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";

import { themes, ThemeName } from '@/lib/themes';
import { useTheme } from "@/lib/ThemeContext";
import { Log } from "@/lib/utils";
import { DictatorConfig, ModelInfo, SimpleResult } from "@/types"
import SelectBox from "@/components/SelectBox";

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
  availableModels: ModelInfo[],
  testConnection: () => Promise<void>, // Make async
  supportsModels: boolean, // Pass support status
}

interface FormButtonProps extends FormProps {
  cancelEdit: () => void,
  saveSettings: () => Promise<void>, // Make async
  toggleEditMode: () => void,
}

const ThemeSelection = ({ settings, handleChange }: FormProps) => {
  const { changeTheme } = useTheme();

  const onChangeTheme = (theme: string) => {
    changeTheme(theme as ThemeName)
    handleChange("theme", theme)
  }

  return (
    <div className="flex justify-between items-center text-sm mb-2">
      <label>Theme</label>
      <div className="w-40"> {/* Fixed width container for consistent sizing */}
        <SelectBox
          value={settings.theme}
          onChange={onChangeTheme}
          disabled={!settings.isEditing}
          options={Object.keys(themes).map((name) => ({
            value: name,
            label: themes[name as ThemeName].name
          }))}
        />
      </div>
    </div>
  )
}

const APISettings = ({
  availableModels,
  settings,
  handleChange,
  testConnection,
  supportsModels, // Receive support status
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

      {/* API URL Input */}
      <div className="mb-2">
        <label className="text-sm block mb-1">API URL</label>
        <input
          type="text"
          value={settings.apiUrl}
          onChange={(e) => handleChange('apiUrl', e.target.value)}
          disabled={!settings.isEditing}
          className="w-full px-3 py-1 rounded" // Removed mt-1, added block to label
          style={{
            backgroundColor: settings.isEditing ? colors.surface0 : colors.mantle, // Adjust bg when disabled
            color: colors.text,
            borderColor: colors.surface1,
            borderWidth: '1px', // Ensure border is visible
            opacity: settings.isEditing ? 1 : 0.7,
          }}
        />
      </div>

      {/* API Key Input */}
      <div className="mb-2">
        <label className="text-sm block mb-1">API Key</label>
        <input
          type="password"
          value={settings.apiKey}
          onChange={(e) => handleChange('apiKey', e.target.value)}
          disabled={!settings.isEditing}
          className="w-full px-3 py-1 rounded"
          style={{
            backgroundColor: settings.isEditing ? colors.surface0 : colors.mantle,
            color: colors.text,
            borderColor: colors.surface1,
            borderWidth: '1px',
            opacity: settings.isEditing ? 1 : 0.7,
          }}
        />
      </div>

      {/* Model Selection/Input */}
      <div className="mb-3">
        <label className="text-sm block mb-1">Model</label>
        {supportsModels && availableModels.length > 0 && settings.isEditing ? (
          <SelectBox
            value={settings.defaultModel}
            onChange={(value) => handleChange('defaultModel', value)}
            options={[
              { value: "", label: "Select Model (Optional)" }, // Add placeholder/optional
              ...availableModels.map(model => ({ value: model.id, label: model.id }))
            ]}
            className="w-full" // Removed mt-1
          />
        ) : (
          <input
            type="text"
            placeholder={supportsModels ? "Enter model ID or leave blank" : "Model selection not supported"}
            value={settings.defaultModel}
            onChange={(e) => handleChange('defaultModel', e.target.value)}
            disabled={!settings.isEditing || !supportsModels} // Disable if not editing OR not supported
            className="w-full px-3 py-1 rounded"
            style={{
              backgroundColor: settings.isEditing && supportsModels ? colors.surface0 : colors.mantle,
              color: colors.text,
              borderColor: colors.surface1,
              borderWidth: '1px',
              opacity: settings.isEditing && supportsModels ? 1 : 0.7,
            }}
          />
        )}
        {!supportsModels && (
          <p className="text-xs mt-1" style={{ color: colors.overlay }}>
            Model listing/selection not supported by this API endpoint.
          </p>
        )}
      </div>

      {/* Test Connection Button */}
      {settings.isEditing && (
        <button
          onClick={testConnection}
          disabled={settings.isSubmitting}
          className="w-[50%] mx-auto mb-3 py-1 px-1 flex rounded-md justify-center items-center" // Removed extra rounded
          style={{
            backgroundColor: colors.surface1,
            color: colors.sky,
            opacity: settings.isSubmitting ? 0.7 : 1,
          }}
        >
          {settings.isSubmitting && settings.testSuccess === null ? ( // Only show spinner during test
            <div
              className="animate-spin h-4 w-4 border-2 rounded-full border-b-transparent mr-2"
              style={{ borderColor: colors.text, borderBottomColor: 'transparent' }}
            />
          ) : null}
          Test Connection
        </button>
      )}

      {/* Status Messages */}
      <div className="h-8 flex items-center justify-center"> {/* Container to prevent layout shifts */}
        {settings.testSuccess === true && !settings.errorMessage && (
          <div
            className="text-sm px-2 py-1 rounded w-auto text-center transition-opacity duration-300"
            style={{
              backgroundColor: colors.green + '20',
              color: colors.green,
              border: `1px solid ${colors.green}30`,
            }}
          >
            Connection Successful
          </div>
        )}
        {settings.errorMessage && (
          <div
            className="text-sm px-2 py-1 rounded w-auto text-center transition-opacity duration-300"
            style={{
              backgroundColor: colors.red + '20',
              color: colors.red,
              border: `1px solid ${colors.red}30`,
            }}
          >
            {settings.errorMessage}
          </div>
        )}
      </div>
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
    <div className="flex flex-col justify-end mb-2 sticky bottom-0 bg-inherit pt-2 px-4">
      <div className="flex justify-end gap-2">
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
    </div>
  )
}

const SettingsPanel = () => {
  const { colors, themeName, changeTheme } = useTheme(); // Get changeTheme
  const [settings, setSettings] = useState<SettingsForm>({
    apiUrl: "",
    apiKey: "",
    defaultModel: "",
    theme: themeName,
    isEditing: false,
    isSubmitting: false,
    errorMessage: "",
    testSuccess: null,
  });
  const [availableModels, setAvailableModels] = useState<ModelInfo[]>([]);
  const [supportsModels, setSupportsModels] = useState<boolean>(false); // Track support
  const [initialSettings, setInitialSettings] = useState<Partial<SettingsForm>>({}); // Store initial settings for cancel

  useEffect(() => {
    fetchSettingsAndSupport();
  }, []);

  const fetchSettingsAndSupport = async () => {
    try {
      Log.d("Fetching settings...");
      const config: DictatorConfig = await invoke("get_settings");
      Log.d("Settings fetched:", config);

      Log.d("Checking model endpoint support...");
      const modelSupport: boolean = await invoke("supports_models_endpoint");
      Log.d("Model endpoint support:", modelSupport);
      setSupportsModels(modelSupport);

      const currentSettings = {
        apiUrl: config.apiUrl,
        apiKey: config.apiKey,
        defaultModel: config.defaultModel,
        theme: config.theme || themeName,
      };

      setSettings(prev => ({
        ...prev,
        ...currentSettings,
        isEditing: false, // Reset editing state on fetch
        errorMessage: "",
        testSuccess: null,
      }));
      setInitialSettings(currentSettings); // Store fetched settings for cancellation

      // Apply fetched theme
      if (config.theme && config.theme !== themeName) {
        changeTheme(config.theme as ThemeName);
      }


      if (modelSupport) {
        fetchModels();
      } else {
        setAvailableModels([]); // Clear models if not supported
      }
    } catch (error) {
      const errorMsg = `Failed to load settings or check support: ${error instanceof Error ? error.message : String(error)}`;
      setSettings(prev => ({
        ...prev,
        errorMessage: errorMsg,
      }));
      Log.e(errorMsg);
    }
  };


  const fetchModels = async () => {
    try {
      Log.d("Fetching available models...");
      const models: ModelInfo[] = await invoke("list_available_models");
      Log.d("Models fetched:", models);
      setAvailableModels(models);
    } catch (error) {
      // Don't necessarily show this as a main error unless testing connection
      Log.e("Failed to fetch models:", error);
      setAvailableModels([]); // Ensure models are cleared on error
    }
  };

  const handleChange = (field: keyof SettingsForm, value: string) => {
    setSettings(prev => ({
      ...prev,
      [field]: value,
      errorMessage: "", // Clear errors on change
      testSuccess: null, // Reset test status on change
    }));
  };

  const toggleEditMode = () => {
    setSettings(prev => ({
      ...prev,
      isEditing: !prev.isEditing,
      errorMessage: "",
      testSuccess: null,
    }));
    // If entering edit mode, ensure models are fetched if supported
    if (!settings.isEditing && supportsModels) {
      fetchModels();
    }
  };

  const testConnection = async () => {
    setSettings(prev => ({ ...prev, isSubmitting: true, errorMessage: "", testSuccess: null }));
    Log.d("Testing connection with settings:", settings);

    try {
      // We don't need to save temporarily. Just try listing models.
      // The `list_available_models` command uses the *managed state* client,
      // which hasn't been updated yet. This isn't ideal for testing *unsaved* settings.
      // A better approach would be a dedicated 'test_connection' command in Rust
      // that takes the settings as arguments.

      // --- Workaround: Temporarily update state for test ---
      // This is slightly hacky but avoids adding a new Rust command just for testing.
      const currentConfig = {
        apiUrl: settings.apiUrl,
        apiKey: settings.apiKey,
        defaultModel: settings.defaultModel,
        theme: settings.theme,
      };
      // Temporarily save to update the backend state for the test call
      await invoke("save_settings", { settings: currentConfig });

      try {
        const modelSupport: boolean = await invoke("supports_models_endpoint");
        setSupportsModels(modelSupport); // Update support status based on test
        if (modelSupport) {
          await invoke("list_available_models"); // Try listing models
          Log.i("Test connection successful (models supported).");
          setSettings(prev => ({ ...prev, testSuccess: true, errorMessage: "" }));
        } else {
          Log.i("Test connection successful (models not supported).");
          setSettings(prev => ({ ...prev, testSuccess: true, errorMessage: "API OK, models endpoint not found." }));
        }
      } catch (listError) {
        // If listing models fails even after a successful save_settings,
        // it implies a connection issue or API incompatibility.
        throw new Error(`Connection failed or API incompatible: ${listError instanceof Error ? listError.message : String(listError)}`);
      } finally {
        // --- Restore original settings in backend state ---
        await invoke("save_settings", { settings: initialSettings });
        Log.d("Restored original settings after test.");
      }
      setSettings(prev => ({ ...prev, isSubmitting: false }));

    } catch (error) {
      const errorMsg = `Connection test failed: ${error instanceof Error ? error.message : String(error)}`;
      Log.e(errorMsg);
      setSettings(prev => ({
        ...prev,
        isSubmitting: false,
        testSuccess: false,
        errorMessage: errorMsg,
      }));
      // --- Restore original settings in backend state on error too ---
      try {
        await invoke("save_settings", { settings: initialSettings });
        Log.d("Restored original settings after test failure.");
      } catch (restoreError) {
        Log.e("Failed to restore original settings after test failure:", restoreError);
      }
    }
  };


  const saveSettings = async () => {
    setSettings(prev => ({ ...prev, isSubmitting: true, errorMessage: "" }));
    Log.d("Saving settings:", settings);

    try {
      // Prepare the payload matching the Rust struct DictatorConfig
      const newSettingsPayload: DictatorConfig = {
        apiUrl: settings.apiUrl,
        apiKey: settings.apiKey,
        defaultModel: settings.defaultModel,
        theme: settings.theme,
        // supportsModels is not part of the saved config
      };

      const result: SimpleResult = await invoke("save_settings", { settings: newSettingsPayload });

      if (!result.success) {
        throw new Error(result.error || "Failed to save settings");
      }

      Log.i("Settings saved successfully.");
      // Update initial settings to reflect the save
      setInitialSettings({
        apiUrl: settings.apiUrl,
        apiKey: settings.apiKey,
        defaultModel: settings.defaultModel,
        theme: settings.theme,
      });
      // Exit edit mode
      setSettings(prev => ({
        ...prev,
        isSubmitting: false,
        isEditing: false,
        errorMessage: "",
        testSuccess: null, // Clear test status
      }));
      // Re-check model support with new settings
      const newSupport = await invoke("supports_models_endpoint");
      setSupportsModels(newSupport as boolean);
      if (newSupport) fetchModels(); else setAvailableModels([]);

    } catch (error) {
      const errorMsg = `Save settings failed: ${error instanceof Error ? error.message : String(error)}`;
      Log.e(errorMsg);
      setSettings(prev => ({
        ...prev,
        isSubmitting: false,
        // Keep editing mode on failure? Optional.
        // isEditing: false,
        errorMessage: errorMsg,
      }));
    }
  };

  const cancelEdit = () => {
    Log.d("Cancelling edit, restoring settings:", initialSettings);
    // Reset form to stored initial settings and exit edit mode
    setSettings(prev => ({
      ...prev,
      apiUrl: initialSettings.apiUrl || "",
      apiKey: initialSettings.apiKey || "",
      defaultModel: initialSettings.defaultModel || "",
      theme: initialSettings.theme || themeName,
      isEditing: false,
      errorMessage: "",
      testSuccess: null,
    }));
    // Re-apply the initial theme if it changed during edit
    if (initialSettings.theme && initialSettings.theme !== themeName) {
      changeTheme(initialSettings.theme as ThemeName);
    }
  };

  // JSX structure remains the same, but pass supportsModels to APISettings
  return (
    <div
      className="flex flex-col h-full"
      style={{ maxHeight: "calc(400px - 60px)" }}
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
        <ThemeSelection settings={settings} handleChange={handleChange} />
        <APISettings
          availableModels={availableModels}
          settings={settings}
          handleChange={handleChange}
          testConnection={testConnection}
          supportsModels={supportsModels} // Pass down support status
        />
      </div>

      <FormButtons
        settings={settings}
        cancelEdit={cancelEdit}
        handleChange={handleChange} // Pass down handleChange
        saveSettings={saveSettings}
        toggleEditMode={toggleEditMode}
      />
    </div>
  );
};

export default SettingsPanel;
