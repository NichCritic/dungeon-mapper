#!/bin/bash
set -e

echo "Building Windows binary via Docker..."
docker build -f Dockerfile.windows -t dungeon-drafter-windows .

echo "Extracting binary..."
docker create --name dd-win-extract dungeon-drafter-windows > /dev/null 2>&1
docker cp dd-win-extract:/app/target/x86_64-pc-windows-gnu/release/dungeon-drafter.exe ./dungeon-drafter.exe
docker rm dd-win-extract > /dev/null 2>&1

echo "Done: dungeon-drafter.exe ($(du -h dungeon-drafter.exe | cut -f1))"
