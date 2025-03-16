// src/lib/themes.ts
export const themes = {
  catppuccinMocha: {
    name: "Catppuccin Mocha",
    colors: {
      base: "#1e1e2e",
      mantle: "#181825",
      crust: "#11111b",
      text: "#cdd6f4",
      subtext: "#a6adc8",
      overlay: "#6c7086",
      surface0: "#313244",
      surface1: "#45475a",
      surface2: "#585b70",
      blue: "#89b4fa",
      lavender: "#b4befe",
      sapphire: "#74c7ec",
      sky: "#89dceb",
      teal: "#94e2d5",
      green: "#a6e3a1",
      yellow: "#f9e2af",
      peach: "#fab387",
      maroon: "#eba0ac",
      red: "#f38ba8",
      mauve: "#cba6f7",
      pink: "#f5c2e7",
      flamingo: "#f2cdcd",
      rosewater: "#f5e0dc",
      accent: "#f5c2e7", // mauve as default accent
      highlight: "#89b4fa", // blue as highlight
    },
  },
};

export type ThemeName = keyof typeof themes;
