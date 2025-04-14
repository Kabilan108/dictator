
// src/types.ts

// Matches the Rust struct DictatorConfig in config.rs
export interface DictatorConfig {
  apiUrl: string;
  apiKey: string;
  defaultModel: string;
  theme: string;
  // Add supportsModels if you decide to pass it from get_settings in Rust
  // supportsModels?: boolean;
}

// Matches the Rust struct ModelInfo in whisper.rs
export interface ModelInfo {
  id: string;
}

// Matches the Rust struct SimpleResult in commands.rs
export interface SimpleResult {
  success: boolean;
  error?: string; // Optional error message
}

// Matches the Rust struct TranscriptionResult in commands.rs
export interface TranscriptionResult {
  success: boolean;
  transcript?: string; // Optional transcript
  error?: string;      // Optional error message
}

// You can add other shared types here if needed
