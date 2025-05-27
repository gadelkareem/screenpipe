#!/usr/bin/env bash
set -e


# https://github.com/mediar-ai/screenpipe/blob/main/CONTRIBUTING.md

git clone https://github.com/mediar-ai/screenpipe
cd screenpipe
brew install pkg-config ffmpeg jq cmake wget rustup-init bun 
sudo xcodebuild -license
xcodebuild -runFirstLaunch
cargo build --release --features metal


