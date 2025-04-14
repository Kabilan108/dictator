// src/lib/themes.ts
//
// implement a catpuccin latte theme
//
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
  catppuccinLatte: {
    name: "Catppuccin Latte",
    colors: {
      base: "#eff1f5",
      mantle: "#e6e9ef",
      crust: "#dce0e8",
      text: "#4c4f69",
      subtext: "#5c5f77",
      overlay: "#6c6f85",
      surface0: "#ccd0da",
      surface1: "#bcc0cc",
      surface2: "#acb0be",
      blue: "#1e66f5",
      lavender: "#7287fd",
      sapphire: "#209fb5",
      sky: "#04a5e5",
      teal: "#179299",
      green: "#40a02b",
      yellow: "#df8e1d",
      peach: "#fe640b",
      maroon: "#e64553",
      red: "#d20f39",
      mauve: "#8839ef",
      pink: "#ea76cb",
      flamingo: "#dd7878",
      rosewater: "#dc8a78",
      accent: "#8839ef",
      highlight: "#1e66f5",
    },
  },
};

export type ThemeName = keyof typeof themes;
