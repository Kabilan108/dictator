
import { Check } from "lucide-react";
import { themes, ThemeName } from '@/lib/themes';
import { useTheme } from "@/lib/ThemeContext";

export function SettingsPanel() {
  const { themeName, colors, changeTheme } = useTheme();

  return (
    <>
      <div className="flex mt-2 mb-2">
        <h3 className="font-medium">Settings</h3>
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
    </>
  );
}

