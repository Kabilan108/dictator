# Dictator Project Guide

## Build Commands
- **Build app**: `wails build --clean`
- **Build macOS**: `./scripts/build-macos.sh`
- **Build Windows**: `./scripts/build-windows.sh`
- **Frontend dev**: `cd frontend && npm run dev`
- **Frontend build**: `cd frontend && npm run build`
- **Frontend lint**: `cd frontend && npm run lint`

## Code Style Guidelines
### Go
- **Naming**: PascalCase for exported, camelCase for internal
- **Error handling**: Use `fmt.Errorf("context: %w", err)` wrapping
- **Logging**: Use app.Log.[DIWE] methods for different levels
- **Config**: app.GetConfigString/SaveConfig for persistent settings

### TypeScript/React
- **Format**: Use Biome (88 char line width, 2-space indent, double quotes)
- **Types**: Always define explicit types, especially for state and props
- **Components**: PascalCase named exports in dedicated files
- **Hooks**: Prefer useCallback for functions passed as props
- **Error handling**: Try/catch with Log.e for API calls

## Project Structure
- **Go backend**: Wails desktop app with whisper.cpp integration
- **React frontend**: Tailwind/shadcn UI with theme support
- **Whisper API**: OpenAI-compatible transcription service
