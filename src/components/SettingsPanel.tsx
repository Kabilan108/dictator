import { themes, ThemeName } from '@/lib/themes';
import { useTheme } from "@/contexts/ThemeContext";
import { DictatorConfig, ModelInfo } from "@/types"
import SelectBox from "@/components/SelectBox";
import { useSettings } from "@/contexts/SettingsContext";

const SelectTheme = ({ isEditing, settings, handleChange }: {
  isEditing: boolean,
  settings: DictatorConfig,
  handleChange: (field: keyof DictatorConfig, value: string) => void,
}) => {
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
          disabled={!isEditing}
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
  isEditing,
  isSubmitting,
  errorMessage,
  models,
  settings,
  supportsModels,
  testSuccess,
  handleChange,
  testConnection,
}: {
  isEditing: boolean,
  isSubmitting: boolean,
  errorMessage: string,
  models: ModelInfo[],
  settings: DictatorConfig,
  supportsModels: boolean,
  testSuccess: boolean | null,
  handleChange: (field: keyof DictatorConfig, value: string) => void,
  testConnection: () => Promise<void>,
}) => {
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
          disabled={!isEditing}
          className="w-full px-3 py-1 rounded" // Removed mt-1, added block to label
          style={{
            backgroundColor: isEditing ? colors.surface0 : colors.mantle, // Adjust bg when disabled
            color: colors.text,
            borderColor: colors.surface1,
            borderWidth: '1px', // Ensure border is visible
            opacity: isEditing ? 1 : 0.7,
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
          disabled={!isEditing}
          className="w-full px-3 py-1 rounded"
          style={{
            backgroundColor: isEditing ? colors.surface0 : colors.mantle,
            color: colors.text,
            borderColor: colors.surface1,
            borderWidth: '1px',
            opacity: isEditing ? 1 : 0.7,
          }}
        />
      </div>

      {/* Model Selection/Input */}
      <div className="mb-3">
        <label className="text-sm block mb-1">Model</label>
        {supportsModels && models.length > 0 && isEditing ? (
          <SelectBox
            value={settings.defaultModel}
            onChange={(value) => handleChange('defaultModel', value)}
            options={[
              { value: "", label: "Select Model (Optional)" }, // Add placeholder/optional
              ...models.map(model => ({ value: model.id, label: model.id }))
            ]}
            className="w-full" // Removed mt-1
          />
        ) : (
          <input
            type="text"
            placeholder={supportsModels ? "Enter model ID or leave blank" : "Model selection not supported"}
            value={settings.defaultModel}
            onChange={(e) => handleChange('defaultModel', e.target.value)}
            disabled={!isEditing || !supportsModels} // Disable if not editing OR not supported
            className="w-full px-3 py-1 rounded"
            style={{
              backgroundColor: isEditing && supportsModels ? colors.surface0 : colors.mantle,
              color: colors.text,
              borderColor: colors.surface1,
              borderWidth: '1px',
              opacity: isEditing && supportsModels ? 1 : 0.7,
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
      {isEditing && (
        <button
          onClick={testConnection}
          disabled={isSubmitting}
          className="w-[50%] mx-auto mb-3 py-1 px-1 flex rounded-md justify-center items-center" // Removed extra rounded
          style={{
            backgroundColor: colors.surface1,
            color: colors.sky,
            opacity: isSubmitting ? 0.7 : 1,
          }}
        >
          {isSubmitting && testSuccess === null ? ( // Only show spinner during test
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
        {testSuccess === true && !errorMessage && (
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
        {errorMessage && (
          <div
            className="text-sm px-2 py-1 rounded w-auto text-center transition-opacity duration-300"
            style={{
              backgroundColor: colors.red + '20',
              color: colors.red,
              border: `1px solid ${colors.red}30`,
            }}
          >
            {errorMessage}
          </div>
        )}
      </div>
    </div>
  )
}

const FormButtons = ({
  isEditing,
  isSubmitting,
  cancelEdit,
  saveSettings,
  toggleEditMode,
}: {
  isEditing: boolean,
  isSubmitting: boolean,
  settings: DictatorConfig,
  cancelEdit: () => void;
  saveSettings: () => Promise<void>;
  toggleEditMode: () => void;
}) => {
  const { colors } = useTheme();

  return (
    <div className="flex flex-col justify-end mb-2 sticky bottom-0 bg-inherit pt-2 px-4">
      <div className="flex justify-end gap-2">
        {!isEditing ? (
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
              disabled={isSubmitting}
              className="py-1 px-5 rounded"
              style={{
                backgroundColor: colors.accent,
                color: colors.base
              }}
            >
              {isSubmitting ? 'Saving...' : 'Save'}
            </button>
          </>
        )}
      </div>
    </div>
  )
}

const SettingsPanel = () => {
  const { colors } = useTheme();
  const {
    settings,
    models,
    supportsModels,
    isEditing,
    isSubmitting,
    errorMessage,
    testSuccess,
    handleChange,
    toggleEditMode,
    cancelEdit,
    saveSettings,
    testConnection,
  } = useSettings();

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
        <SelectTheme isEditing={isEditing} settings={settings} handleChange={handleChange} />
        <APISettings
          isEditing={isEditing}
          isSubmitting={isSubmitting}
          errorMessage={errorMessage}
          models={models}
          settings={settings}
          supportsModels={supportsModels}
          testSuccess={testSuccess}
          handleChange={handleChange}
          testConnection={testConnection}
        />
      </div>
      <FormButtons
        isEditing={isEditing}
        isSubmitting={isSubmitting}
        settings={settings}
        cancelEdit={cancelEdit}
        saveSettings={saveSettings}
        toggleEditMode={toggleEditMode}
      />
    </div>
  );
};

export default SettingsPanel;
