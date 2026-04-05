# Dungeon Mapper

A Rust desktop application for creating, styling, and presenting D&D dungeon maps. Built with egui/eframe.

## Overview

Dungeon Mapper provides a full workflow for building dungeon maps from abstract topology graphs through to themed, presentation-ready renders with fog of war and combat tracking.

## Tech Stack

- **Rust** with **egui 0.31** / **eframe** for the UI
- **petgraph** for the room/connection graph model
- **serde** / **serde_json** for serialization (versioned `.dungeon` save format)
- **image** crate for PNG export
- **rfd** for native file dialogs
- **tungstenite** for a WebSocket-based presentation server

## Workflow

The app is organized into five tabs that form a pipeline from topology to final output:

### 1. Graph

Define rooms and connections as a topology graph. Double-click to add rooms, right-drag to connect them. Supports copy/paste, multi-select, and grouping.

### 2. Spatial

Auto-solve the graph into a 2D spatial layout. Drag rooms and corridors to refine positioning. Features waypoints, exit handles, elevation sections, and floor filtering.

### 3. Decor

Place decorative objects inside rooms (tables, chairs, etc.). Drag to position, right-drag to box-select. Manages light sources and ambient lighting.

### 4. Encounters

Set up combat encounters using the 5e-Tools bestiary database. Includes a monster browser with search and filtering, custom monster creation, a monster merging workshop, and import/export for encounters and creatures. Supports hazard encounters with save DC and effects.

### 5. Styled

Final themed rendering with exterior shading styles (solid, hatched, stippled), grid overlay, and labels. Export DM and player maps as PNG.

## Presentation Mode

Run your dungeon at the table with built-in presentation tools:

- **Fog of war** with per-room visibility (Hidden / Explored / Visible)
- **Door state tracking** (open/closed)
- **Party token** placement
- **Wandering encounter** simulation with autobattle
- **Combat tracker** with initiative, HP, conditions, and attack rolls
- **Combat simulator** (1v1 and free-for-all)
- **Player view** in a second window (for a second screen)
- **WebSocket server** to push the player view to browsers on other devices

## Other Features

- Background render caching for responsive UI on large maps
- Undo/redo (`Ctrl+Z` / `Ctrl+Y`)
- Auto-save
- Annotation system (`F7`) for leaving notes on the map
- Help overlay (`F8`) for contextual UI help
- 5e-Tools bestiary integration with full stat blocks and `_copy` resolution
- Versioned save format (`.dungeon` files)

## Building

```
cargo build --release
```

Requires the `5etools-src/data/bestiary/` directory populated with bestiary JSON files for the monster database (loaded in the background on startup).

## Project Structure

```
src/
  app.rs          - Main application state and frame loop
  main.rs         - Entry point
  history.rs      - Undo/redo system
  model/          - Data model (rooms, connections, encounters, etc.)
  solver/         - Graph-to-spatial layout solver
  render/         - Map rendering (styled output, shading, grid)
  ui/             - Tab UIs (graph, spatial, decor, encounters, styled)
  presentation/   - Fog of war, combat tracker, player view
  server/         - WebSocket presentation server
  io/             - Save/load, PNG export, import/export
  data/           - Bestiary loading and monster data
  util/           - Shared utilities
```

## Status

This is a personal project under active development.
