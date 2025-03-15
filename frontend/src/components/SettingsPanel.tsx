// src/components/SettingsPanel.tsx
import { useTheme } from "@/lib/ThemeContext";
import { themes, ThemeName } from "@/lib/themes";
import { Check, X } from "lucide-react";

interface SettingsPanelProps {
  onClose: () => void;
}

export function SettingsPanel({ onClose }: SettingsPanelProps) {
  const { themeName, colors, changeTheme } = useTheme();

  return (
    <div
      className="absolute inset-0 z-10 flex flex-col p-4"
      style={{ backgroundColor: colors.base }}
    >
      <div className="flex justify-between items-center mb-4">
        <h3 className="font-medium">Settings</h3>
        <button
          onClick={onClose}
          className="p-1 hover:opacity-80 transition-opacity"
          style={{ color: colors.overlay }}
        >
          <X size={16} />
        </button>
      </div>

      <div className="mb-4">
        <h4
          className="text-sm mb-2 pb-1 border-b"
          style={{ borderColor: colors.surface1 }}
        >
          Theme
        </h4>
        {Object.keys(themes).map((name) => (
          <button
            key={name}
            onClick={() => changeTheme(name as ThemeName)}
            className="flex items-center w-full px-2 py-1.5 rounded mb-1 hover:cursor-pointer"
            style={{
              backgroundColor: themeName === name ? colors.surface0 : 'transparent',
            }}
          >
            <div
              className="w-4 h-4 rounded-full mr-2"
              style={{ backgroundColor: themes[name as ThemeName].colors.accent }}
            />
            <span className="flex-1 text-left text-sm">{themes[name as ThemeName].name}</span>
            {themeName === name && (
              <Check size={14} style={{ color: colors.green }} />
            )}
          </button>
        ))}
      </div>
    </div>
  );
}
