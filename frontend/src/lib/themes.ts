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
      accent: "#cba6f7", // mauve as default accent
      highlight: "#89b4fa", // blue as highlight
    },
  },
  gruvbox: {
    name: "Gruvbox Dark",
    colors: {
      base: "#282828",
      mantle: "#1d2021",
      crust: "#1d2021",
      text: "#ebdbb2",
      subtext: "#d5c4a1",
      overlay: "#928374",
      surface0: "#3c3836",
      surface1: "#504945",
      surface2: "#665c54",
      blue: "#83a598",
      lavender: "#d3869b",
      sapphire: "#458588",
      sky: "#83a598",
      teal: "#8ec07c",
      green: "#b8bb26",
      yellow: "#fabd2f",
      peach: "#fe8019",
      maroon: "#cc241d",
      red: "#fb4934",
      mauve: "#d3869b",
      pink: "#d3869b",
      flamingo: "#fe8019",
      rosewater: "#ebdbb2",
      accent: "#fe8019", // peach as default accent
      highlight: "#fabd2f", // yellow as highlight
    },
  },
};

export type ThemeName = keyof typeof themes;
