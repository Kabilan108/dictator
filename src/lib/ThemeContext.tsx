// src/lib/ThemeContext.tsx
import { createContext, useState, useContext, ReactNode } from 'react';
import { themes, ThemeName } from '@/lib/themes';

type ThemeContextType = {
  themeName: ThemeName;
  colors: typeof themes.catppuccinMocha.colors;
  changeTheme: (name: ThemeName) => void;
};

const ThemeContext = createContext<ThemeContextType | undefined>(undefined);

export const ThemeProvider = ({ children }: { children: ReactNode }) => {
  const [themeName, setThemeName] = useState<ThemeName>('catppuccinMocha');

  const changeTheme = (name: ThemeName) => {
    setThemeName(name);
  };

  const value = {
    themeName,
    colors: themes[themeName].colors,
    changeTheme,
  };

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
};

export const useTheme = () => {
  const context = useContext(ThemeContext);
  if (context === undefined) {
    throw new Error('useTheme must be used within a ThemeProvider');
  }
  return context;
};
