#!/usr/bin/env bash
set -e

# Base command for screenpipe
SCREENPIPE_COMMAND="./target/release/screenpipe"

DYNAMIC_IGNORED_ARGS_STRING=""

# Process the first argument if it exists (comma-separated keywords)
if [[ -n "$1" ]]; then
  KEYWORDS_CSV="$1"
  
  _OLD_IFS="$IFS"
  IFS=','
  # Read the comma-separated values into an array
  read -r -a KEYWORDS_ARRAY <<< "$KEYWORDS_CSV"
  IFS="$_OLD_IFS"
  
  for keyword in "${KEYWORDS_ARRAY[@]}"; do
    # Trim leading/trailing whitespace from each keyword
    trimmed_keyword=$(echo "$keyword" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')
    if [[ -n "$trimmed_keyword" ]]; then
      # Append each keyword as a properly quoted --ignored-windows argument,
      # followed by a line continuation and newline for the FINAL_COMMAND string.
      DYNAMIC_IGNORED_ARGS_STRING+="    --ignored-windows \"$trimmed_keyword\" "
    fi
  done
  shift # Remove the processed first argument, $@ now contains the rest
fi

# Construct the final command string for screenpipe
# If DYNAMIC_IGNORED_ARGS_STRING is not empty, it ends with ' \\', so the next line continues correctly.
# If DYNAMIC_IGNORED_ARGS_STRING is empty, the line with .env simply continues to --fps.
FINAL_COMMAND="$SCREENPIPE_COMMAND \\
    --disable-telemetry \\
    --audio-transcription-engine whisper-large-v3-turbo \\
    --ocr-engine apple-native \\
    --ignored-windows \"Private\" \\
    --ignored-windows \"Keepass\" \\
    --ignored-windows \"Vaults\" \\
    --ignored-windows \".env\" \\
    --fps 0.5 \\
    --enable-frame-cache \\
    --enable-pipe-manager \\
    $DYNAMIC_IGNORED_ARGS_STRING"

# For debugging purposes: echo the command that would be run.
# Append any remaining script arguments (originally $2 onwards)
# echo "Executing: $FINAL_COMMAND $@"

# Execute the screenpipe command, passing through any additional arguments
# (originally $2 onwards)
eval "$FINAL_COMMAND \"\$@\""

