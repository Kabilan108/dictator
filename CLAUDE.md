# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Development Commands
- Build: `npm run build` or `yarn build`
- Dev server: `npm run dev` or `yarn dev`
- Tauri: `npm run tauri` or `yarn tauri`
- Lint: `npx @biomejs/biome check src/` (using Biome.js)
- Format: `npx @biomejs/biome format src/ --write`

## Code Style Guidelines
- **Formatting**: Use 2-space indentation, 88 char line width, double quotes
- **Imports**: Use path aliases (`@/*`), organize imports automatically
- **TypeScript**: Strict mode enabled, prevent unused variables/parameters
- **React**: Functional components with hooks, avoid class components
- **Error handling**: Use proper error boundaries and typed error handling
- **Naming**: camelCase for variables/functions, PascalCase for components/types

## Architecture Notes
- Frontend: React + TypeScript + Tailwind CSS + Radix UI
- Backend: Tauri with Rust
- Audio processing: Uses whisper.cpp for speech-to-text functionality
