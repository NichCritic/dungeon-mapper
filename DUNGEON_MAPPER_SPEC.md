# Dungeon Drafter — Project Specification

## Vision

A desktop dungeon design tool built around a **three-stage abstraction pipeline**: Graph → Spatial → Styled. DMs author dungeons structurally (as a node graph), then spatially (as constrained geometry), then visually (with applied themes). Each stage is independently editable without breaking the others.

This is NOT "paint but for dungeons." It's a drafting tool that treats dungeon design as an architectural problem.

---

## Tech Stack

- **Language**: Rust
- **GUI**: egui via eframe — immediate-mode GUI, excellent for tool/editor UIs with custom canvas drawing
- **Graph data structures**: petgraph — mature graph library with BFS, adjacency lists, etc.
- **Serialization**: serde + serde_json — save/load dungeon files as JSON
- **Image export**: image crate — render to PNG
- **File dialogs**: rfd (rusty file dialogs) — native open/save dialogs
- **IDs**: uuid crate

No web tech. No Electron/Tauri. Pure native Rust.

---

## Data Model

This is the core of the whole system. The graph, spatial, and styled layers share a common dungeon model but contribute different data to it.

```rust
use serde::{Serialize, Deserialize};

// === GRAPH LAYER ===

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Room {
    pub id: String,
    pub label: String,
    pub tags: Vec<RoomTag>,
    pub notes: String,
    pub size_hint: SizeHint,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum RoomTag {
    Entrance,
    Boss,
    Trap,
    Treasure,
    Optional,
    Secret,
    Rest,
    Custom(String),
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub enum SizeHint {
    Small,   // ~15x15 ft (3x3 grid)
    Medium,  // ~20x20 ft (4x4 grid)
    Large,   // ~30x30 ft (6x6 grid)
    Huge,    // ~40x40 ft (8x8 grid)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Connection {
    pub id: String,
    pub connection_type: ConnectionType,
    pub label: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub enum ConnectionType {
    Open,
    Door,
    Locked,
    Secret,
    OneWay,
}

// === SPATIAL LAYER ===

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoomLayout {
    pub room_id: String,         // maps back to Room.id
    pub x: i32,                  // grid position (top-left)
    pub y: i32,
    pub width: u32,              // grid squares (1 square = 5ft)
    pub height: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CorridorSegment {
    pub connection_id: String,   // maps back to Connection.id
    pub waypoints: Vec<GridPos>, // path through the grid
    pub width: u32,              // typically 2 (10ft)
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct GridPos {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpatialLayout {
    pub rooms: Vec<RoomLayout>,
    pub corridors: Vec<CorridorSegment>,
    pub bounds_width: u32,
    pub bounds_height: u32,
}

// === STYLE LAYER ===

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,
    pub wall_color: [u8; 4],     // RGBA
    pub floor_color: [u8; 4],
    pub bg_color: [u8; 4],
    pub wall_style: WallStyle,
    pub grid_visible: bool,
    pub hatching: bool,          // Dyson-style wall hatching
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub enum WallStyle {
    Sharp,    // clean straight lines (keep, dungeon)
    Rough,    // slightly jagged (cave, mine)
    Rounded,  // smooth curves (crypt, temple)
}

// === TOP-LEVEL ===

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Dungeon {
    pub name: String,
    pub graph: DungeonGraph,
    pub layout: Option<SpatialLayout>,
    pub theme: Theme,
}

/// Serializable graph. At runtime, rebuild a petgraph::UnGraph for algorithms.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DungeonGraph {
    pub rooms: Vec<Room>,
    pub connections: Vec<StoredEdge>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredEdge {
    pub source_room_id: String,
    pub target_room_id: String,
    pub connection: Connection,
}
```

### Runtime Graph

For pathfinding and layout, rebuild a `petgraph::UnGraph<String, String>` (room_id nodes, connection_id edges) from `DungeonGraph` on the fly. This keeps serialization simple while giving access to petgraph's BFS, shortest paths, etc.

---

## Application Structure

### Main Window Layout

```
+-----------------------------------------------------+
|  [Graph]  [Spatial]  [Styled]          Dungeon Name  |  <- tab bar
+------------------------------------+----------------+
|                                    |                |
|                                    |  Properties    |
|        Main Canvas                 |  Panel         |
|        (changes per tab)           |                |
|                                    |  - Room/Edge   |
|                                    |    properties  |
|                                    |  - Constraints |
|                                    |  - Theme       |
|                                    |  - Export      |
|                                    |                |
+------------------------------------+----------------+
|  Status: 12 rooms, 15 connections  |  Zoom: 100%    |  <- status bar
+-----------------------------------------------------+
```

The main canvas area is a custom egui painter widget in all three tabs. The properties panel on the right is context-sensitive.

### Tab 1: Graph Editor

**Purpose**: Define abstract topology — what rooms exist, how they connect.

**Canvas rendering** (custom egui paint):
- Rooms drawn as rounded rectangles with label text, colored by primary tag
- Connections drawn as lines/curves between rooms
- Edge style varies by connection type (solid=open, dashed=secret, thick=locked, arrow=one-way)
- Simple auto-layout for visual tidiness (this arranges the GRAPH VIEW, not the dungeon spatial layout)

**Interactions**:
- Double-click empty space -> create new room at cursor
- Drag a room -> reposition it in graph view
- Click room -> select, show properties in sidebar
- Right-click room -> context menu (delete, duplicate, set tags)
- Drag from room edge/handle -> start drawing a connection
- Release on another room -> create connection
- Click a connection line -> select, show properties in sidebar
- Delete key -> remove selected room or connection
- Scroll -> zoom, middle-click drag -> pan

**Properties panel (room selected)**:
- Label (text field)
- Size hint (dropdown: Small/Medium/Large/Huge)
- Tags (multi-select checkboxes + custom tag input)
- Notes (multi-line text area)

**Properties panel (connection selected)**:
- Type (dropdown: Open/Door/Locked/Secret/One-Way)
- Label (optional text field, e.g. "Requires silver key")

**Tag -> Color mapping**:
- Entrance: green
- Boss: red
- Trap: orange
- Treasure: gold/yellow
- Optional: gray
- Secret: purple
- Rest: blue
- Custom/untagged: white/default

### Tab 2: Spatial Layout

**Purpose**: Give the graph physical geometry on a grid.

**Canvas rendering**:
- Grid background (5ft squares)
- Rooms as filled rectangles on the grid, labeled
- Corridors as filled paths between rooms
- Bounding box shown as a dashed outline
- Unplaced rooms (if any) listed in sidebar

**Interactions**:
- On first visit (or when clicking "Solve Layout"), run the layout solver
- Click a room -> select, show spatial properties in sidebar
- Drag a room -> reposition on grid (snap to grid)
- Drag room edges -> resize (snap to grid)
- Corridors auto-reroute when rooms move
- Scroll -> zoom, middle-click drag -> pan

**Properties panel**:
- Selected room: position (x,y), dimensions (w x h in grid squares / feet)
- Constraint controls:
  - Bounding box width x height (in grid squares)
  - Density slider (affects gap between rooms in solver)
  - "Re-solve" button — regenerate layout, does not touch the graph
  - "Reset room" — return selected room to its solved position

### Tab 3: Styled View

**Purpose**: Apply visual theme, preview and export the final map.

**Canvas rendering**:
- Full themed render of the spatial layout
- Walls drawn with theme-appropriate style
- Floors filled with theme color
- Grid lines (if enabled)
- Door/connection icons at room-corridor junctions
- Room labels (if enabled)
- Dyson-style hatching on exterior wall sides (if enabled)

**Properties panel**:
- Theme selector (dropdown, MVP: just "Classic Dungeon")
- Toggles: grid lines, room labels, DM notes, secret doors visible
- Export section:
  - "Export DM Map" -> PNG with all annotations
  - "Export Player Map" -> PNG without notes, secrets hidden
  - Resolution multiplier (1x, 2x, 4x)

---

## Layout Solver

The solver takes a `DungeonGraph` and produces a `SpatialLayout`. This is the algorithmic core.

### MVP Algorithm: BFS Greedy Placer

```
fn solve_layout(graph: &DungeonGraph, bounds: (u32, u32), gap: u32) -> Result<SpatialLayout>

1. Build a petgraph from the DungeonGraph
2. Find the entrance room (first room tagged Entrance, or just the first room)
3. Convert each room's SizeHint to default dimensions:
     Small  -> 3x3
     Medium -> 4x4
     Large  -> 6x6
     Huge   -> 8x8
4. Place entrance room near the left-center of the bounding box
5. BFS from entrance through the graph:
     For each unplaced neighbor of a just-placed room:
       a. Compute candidate positions adjacent to the parent room
          (offset by parent dimensions + gap + corridor width)
          Try directions in order: right, below, left, above
       b. For each candidate, check:
          - Fits within bounding box?
          - No overlap with any already-placed room (including gap)?
       c. Place at first valid position
       d. If no adjacent position works, try positions with increasing offset
       e. If still can't place, mark as "unplaced" and warn the user
6. Route corridors:
     For each connection between two placed rooms:
       a. Find nearest edge points between the two rooms
       b. Create a rectilinear path (L-shaped: horizontal then vertical)
       c. If path collides with another room, add a bend to route around it
7. Return SpatialLayout
```

### Corridor Routing (simple)

For each connection:
1. Compute center of the nearest facing edges of the two rooms
2. Create path: exit point -> horizontal segment -> vertical segment -> entry point
3. Collision check against placed rooms; if blocked, try the opposite L-shape
4. Store as a vector of `GridPos` waypoints

---

## Rendering

### egui Custom Painting

All three tabs share a common pattern: allocate a canvas region with `ui.allocate_painter()`, handle input (pan, zoom, click, drag), then draw.

Maintain a `ViewState` per tab:
```rust
pub struct ViewState {
    pub offset: egui::Vec2,   // pan
    pub zoom: f32,            // zoom level
}
```

Transform: `screen_pos = (world_pos * zoom) + offset`

### Grid Drawing
- 1 grid square = 5ft in-world
- At default zoom, 1 grid square = 20px on screen
- Draw grid lines as thin strokes across the canvas
- Heavier lines every 5 squares (25ft) for readability

### Themed Map Rendering (Classic Dungeon)

Render order:
1. Background fill (off-white / parchment: #f5f0e8)
2. Room floor fills (white)
3. Corridor floor fills (white)
4. Grid lines (light gray, 1px)
5. Room walls (black, 2-3px)
6. Corridor walls (black, 2-3px)
7. Dyson hatching (short perpendicular lines on exterior side of walls)
8. Door icons at connection points
9. Room labels (centered, dark gray)
10. DM notes (smaller text below label, only in DM mode)

### Dyson-Style Hatching

The signature old-school D&D map look. For each wall segment:
1. Determine which side is "exterior" (outside the room)
2. Along the wall, at semi-random intervals (every 4-8px):
   - Draw a short line (6-10px) perpendicular to the wall, extending outward
   - Slight random variation in length and angle for organic feel
3. Hatching fills the space between the wall and the map edge / other rooms

### Door Icons

Draw at the midpoint of the wall segment where a corridor meets a room:
- **Open**: gap in wall, no icon
- **Door**: gap in wall + small arc (like a door swinging open)
- **Locked**: gap in wall + filled rectangle + small "x" mark
- **Secret**: no gap visible in player mode; thin dashed line in DM mode
- **One-way**: gap + arrow pointing in the allowed direction

---

## PNG Export

Use the `image` crate to render to an offscreen buffer:

1. Create an `ImageBuffer<Rgba<u8>>` at desired resolution
2. Replay the same drawing logic used for the styled canvas, but targeting the image buffer instead of egui's painter
3. Save via `image::save_buffer()` or present a save dialog with `rfd`

This means the themed renderer should be abstracted behind a trait:

```rust
pub trait MapRenderer {
    fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: [u8; 4]);
    fn stroke_rect(&mut self, x: f32, y: f32, w: f32, h: f32, width: f32, color: [u8; 4]);
    fn draw_line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, width: f32, color: [u8; 4]);
    fn draw_text(&mut self, text: &str, x: f32, y: f32, size: f32, color: [u8; 4]);
}
```

With `EguiRenderer` (wrapping `egui::Painter`) and `ImageRenderer` (wrapping `image::ImageBuffer`) implementations. All themed drawing code calls through this trait so it works for both live preview and export.

---

## File Format

Dungeons save as `.dungeon` files (just JSON):

```json
{
  "name": "Crypt of the Burning Eye",
  "graph": {
    "rooms": [...],
    "connections": [...]
  },
  "layout": {
    "rooms": [...],
    "corridors": [...],
    "bounds_width": 40,
    "bounds_height": 30
  },
  "theme": {
    "name": "Classic Dungeon"
  }
}
```

Use `rfd::FileDialog` for native open/save dialogs. Auto-save to a temp location periodically.

---

## MVP Scope — "Usable by Monday"

### Must Have
1. **Graph editor** — custom egui canvas: create rooms (double-click), connect them (drag), edit properties in sidebar
2. **Layout solver** — BFS greedy placer: graph -> spatial geometry on a grid
3. **Spatial view** — grid canvas with rooms and corridors, drag to reposition rooms
4. **Classic theme renderer** — black/white, Dyson hatching, grid lines, door icons
5. **PNG export** — with player/DM toggle (secrets hidden in player mode)
6. **Save/Load** — JSON files via native file dialogs

### Nice to Have (post-Monday)
- Multiple themes (cave, crypt, sewer)
- Undo/redo (command pattern on dungeon state)
- Room shape options beyond rectangles (L-shapes, circles)
- Corridor manual editing (drag waypoints)
- Room template library (save and reuse room definitions)
- Force-directed graph auto-layout (for tidying the graph view)
- SVG export
- Constraint solver upgrade (SMT-based, respects complex spatial constraints)

### Explicitly Not in MVP
- Multi-level dungeons
- Fog of war / live session play
- Token/encounter management
- Networking / collaboration

---

## Project Structure

```
dungeon-drafter/
├── Cargo.toml
├── src/
│   ├── main.rs                  # eframe app entry point
│   ├── app.rs                   # Top-level App struct, tab switching
│   ├── model/
│   │   ├── mod.rs
│   │   ├── room.rs              # Room, RoomTag, SizeHint
│   │   ├── connection.rs        # Connection, ConnectionType
│   │   ├── graph.rs             # DungeonGraph, StoredEdge, petgraph rebuild
│   │   ├── spatial.rs           # RoomLayout, CorridorSegment, SpatialLayout
│   │   ├── theme.rs             # Theme, WallStyle, default themes
│   │   └── dungeon.rs           # Top-level Dungeon struct
│   ├── ui/
│   │   ├── mod.rs
│   │   ├── graph_editor.rs      # Graph tab: canvas + interactions
│   │   ├── spatial_view.rs      # Spatial tab: grid canvas + drag
│   │   ├── styled_view.rs       # Styled tab: themed preview
│   │   ├── sidebar.rs           # Context-sensitive property panel
│   │   ├── canvas_common.rs     # Shared pan/zoom/input handling
│   │   └── status_bar.rs        # Bottom bar with stats
│   ├── solver/
│   │   ├── mod.rs
│   │   ├── layout.rs            # BFS greedy placer algorithm
│   │   └── corridor.rs          # Rectilinear corridor routing
│   ├── render/
│   │   ├── mod.rs
│   │   ├── traits.rs            # MapRenderer trait
│   │   ├── egui_renderer.rs     # egui::Painter implementation
│   │   ├── image_renderer.rs    # image crate implementation (for export)
│   │   ├── grid.rs              # Grid drawing helpers
│   │   ├── hatching.rs          # Dyson-style hatching logic
│   │   └── doors.rs             # Door icon drawing
│   ├── io/
│   │   ├── mod.rs
│   │   ├── save_load.rs         # JSON serialization + file dialogs
│   │   └── export.rs            # PNG export pipeline
│   └── util/
│       ├── mod.rs
│       ├── grid_math.rs         # Snap-to-grid, coordinate transforms
│       └── ids.rs               # UUID generation helpers
```

---

## Cargo.toml Dependencies

```toml
[package]
name = "dungeon-drafter"
version = "0.1.0"
edition = "2021"

[dependencies]
eframe = "0.31"
egui = "0.31"
egui_extras = "0.31"
petgraph = "0.7"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
image = "0.25"
rfd = "0.15"
uuid = { version = "1", features = ["v4"] }
rand = "0.8"
```

(Pin exact versions after checking crates.io for latest compatible set.)

---

## Implementation Notes

### Build Order (priority)
1. **Data model** (`model/`) — get all structs compiling with serde derives
2. **App shell** (`app.rs`, `ui/sidebar.rs`) — tabbed layout with empty canvases
3. **Graph editor canvas** — room creation, connection drawing, click/drag interactions
4. **Sidebar property editing** — room and connection fields when selected
5. **Layout solver** — BFS placer, the algorithmic heart
6. **Spatial view canvas** — render solved layout on grid, drag rooms to reposition
7. **Themed renderer** — MapRenderer trait + egui implementation, classic theme
8. **PNG export** — ImageRenderer implementation + file dialog
9. **Save/Load** — JSON serialization + file dialogs
10. **Polish** — validation warnings, keyboard shortcuts

### egui Canvas Pattern

Each editor tab follows this pattern:

```rust
fn graph_editor(ui: &mut egui::Ui, state: &mut AppState) {
    let (response, painter) = ui.allocate_painter(
        ui.available_size(),
        egui::Sense::click_and_drag(),
    );
    let rect = response.rect;

    // Handle input
    handle_pan_zoom(&response, &mut state.view);
    handle_clicks(&response, &painter, state);
    handle_drags(&response, state);

    // Draw
    let transform = ViewTransform::new(state.view.offset, state.view.zoom, rect);
    draw_grid(&painter, &transform);
    draw_rooms(&painter, &transform, &state.dungeon);
    draw_connections(&painter, &transform, &state.dungeon);
    draw_selection(&painter, &transform, &state.selection);
}
```

### Graph Editor: Connection Drawing UX

To draw a connection between rooms:
1. Track a `DragState::ConnectingFrom(room_id)` when drag starts on a room's edge zone
2. While dragging, draw a line from the source room to the cursor
3. On release, hit-test against rooms — if hovering over a different room, create the connection
4. If released on empty space, cancel

### Graph Editor: Room Visual Position vs Spatial Position

Important: rooms have TWO positions. Their visual position in the graph editor (for readability) is stored separately from their spatial grid position (for the actual dungeon layout). The graph view position is purely cosmetic — just an (x, y) float for where the node renders in the graph editor canvas. Don't conflate these.

```rust
pub struct GraphViewState {
    pub room_positions: HashMap<String, egui::Pos2>, // graph editor visual positions
    pub view: ViewState,                              // pan/zoom
}
```

### Spatial Room Dragging

When dragging a room in the spatial view:
1. Snap position to grid on every frame
2. Check for overlaps with other rooms — if overlapping, tint red but allow placement
3. After releasing, re-route all corridors connected to that room

### Design Direction

The UI should feel like a professional drafting tool — clean, utilitarian, focused.
- Use egui's default dark theme as the base
- Canvas areas get a lighter background (the "paper")
- Monospace font for room labels and measurements
- Minimal decoration — the dungeon map is the visual focus
- Color in the UI comes from room tags and connection types, not from UI chrome
