#! /bin/bash

log_error() {
  echo "ERROR: $1" >&2
}

if [ -z "${1:-}" ]; then # Check if $1 is set and not empty
  log_error "Pipe ID argument is missing."
  log_error "Usage: $0 <pipe_id>"
  exit 1
fi

pipe_id="$1"
pipe_source_url="https://github.com/mediar-ai/screenpipe/tree/main/pipes/${pipe_id}"
SCREENPIPE_API_PORT="${SCREENPIPE_API_PORT:-3030}" # Default port, can be overridden by env var

cd pipes/${pipe_id}

echo "Requesting download/update for pipe '${pipe_id}' from ${pipe_source_url} via localhost:${SCREENPIPE_API_PORT}"
curl -X POST "http://localhost:${SCREENPIPE_API_PORT}/pipes/download" \
  -H "Content-Type: application/json" \
  -d "{\"url\": \"${pipe_source_url}\"}"

echo "Installing dependencies and building pipe: ${pipe_id}"
bun install && bun run build

# reactivate pipe
echo "Reactivating pipe '${pipe_id}' via localhost:3030"
curl -X POST "http://localhost:${SCREENPIPE_API_PORT}/pipes/disable" \
  -H "Content-Type: application/json" \
  -d "{\"pipe_id\": \"${pipe_id}\"}"
curl -X POST "http://localhost:${SCREENPIPE_API_PORT}/pipes/enable" \
  -H "Content-Type: application/json" \
  -d "{\"pipe_id\": \"${pipe_id}\"}"

pipe_info_response=$(curl -s "http://localhost:${SCREENPIPE_API_PORT}/pipes/info/${pipe_id}")

pipe_running_port=$(echo "${pipe_info_response}" | jq -r '.data.port')

echo "Pipe '${pipe_id}' is running on: http://localhost:${pipe_running_port}"



echo "Pipe '${pipe_id}' processing finished."




