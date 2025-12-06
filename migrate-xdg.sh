#!/bin/bash

# Migration script for Dictator XDG Base Directory compliance
# This script migrates data from old cache-based structure to new XDG structure

set -e

echo "Starting Dictator migration to XDG Base Directory specification..."

# Create new directories
echo "Creating new directories..."
mkdir -p ~/.local/{share,state}/dictator

# Check if old cache directory exists
OLD_CACHE_DIR="$HOME/.cache/dictator"
if [ ! -d "$OLD_CACHE_DIR" ]; then
  echo "Warning: Old cache directory $OLD_CACHE_DIR not found. Nothing to migrate."
  exit 0
fi

# Copy database and recordings
echo "Copying database and recordings..."
if [ -f "$OLD_CACHE_DIR/app.db" ]; then
  cp "$OLD_CACHE_DIR/app.db" ~/.local/share/dictator/
  echo "✓ Copied database"
else
  echo "Warning: Database file not found at $OLD_CACHE_DIR/app.db"
fi

if [ -d "$OLD_CACHE_DIR/recordings" ]; then
  cp -r "$OLD_CACHE_DIR/recordings" ~/.local/share/dictator/
  echo "✓ Copied recordings directory"
else
  echo "Warning: Recordings directory not found at $OLD_CACHE_DIR/recordings"
fi

# Copy log file
echo "Copying log file..."
if [ -f "$OLD_CACHE_DIR/app.log" ]; then
  cp "$OLD_CACHE_DIR/app.log" ~/.local/state/dictator/
  echo "✓ Copied log file"
else
  echo "Warning: Log file not found at $OLD_CACHE_DIR/app.log"
fi

# Update database paths using sqlite-utils
echo "Updating database paths..."
DB_PATH="$HOME/.local/share/dictator/app.db"

if [ -f "$DB_PATH" ]; then
  uvx sqlite-utils convert "$DB_PATH" transcripts audio_path '
        return value.replace("/.cache/", "/.local/share/")
    ' --where "audio_path LIKE '%/.cache/%'"

  echo "✓ Updated database paths"
else
  echo "Warning: Database not found at $DB_PATH, skipping path updates"
fi

echo ""
echo "Migration completed successfully!"
echo ""
echo "New locations:"
echo "  Database: ~/.local/share/dictator/app.db"
echo "  Recordings: ~/.local/share/dictator/recordings/"
echo "  Logs: ~/.local/state/dictator/app.log"
echo ""
echo "You can now remove the old cache directory if desired:"
echo "  rm -rf ~/.cache/dictator"
