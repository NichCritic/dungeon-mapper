#!/usr/bin/env python3
"""
Preview renderer v3: thin border walls, tile-stamped floors, continuous overlays.
Vectorized with numpy for performance.

Walls are thin borders on floor edges, not grid cells.
2.5D faces hang into the void on south/east edges.
Layout uses only 0 (void) and 1 (floor).
"""

import json
import sys
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFilter
from scipy.ndimage import distance_transform_edt

TILE = 64
WALL_THICKNESS = 6
WALL_FACE_HEIGHT = 20

# Two rooms + corridor + side room
LAYOUT = np.array([
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0],
    [0, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0],
    [0, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0],
    [0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0],
    [0, 1, 1, 1, 1, 1, 0, 0, 1, 1, 1, 1, 0],
    [0, 1, 1, 1, 1, 1, 0, 0, 1, 1, 1, 1, 0],
    [0, 0, 0, 1, 0, 0, 0, 0, 1, 1, 1, 1, 0],
    [0, 0, 0, 1, 0, 0, 0, 0, 1, 1, 1, 1, 0],
    [0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0],
    [0, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0],
    [0, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0],
    [0, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
], dtype=np.int32)

ROWS, COLS = LAYOUT.shape
PX_H = ROWS * TILE
PX_W = COLS * TILE


# ---------------------------------------------------------------------------
# Vectorized noise (no per-pixel Python loops)
# ---------------------------------------------------------------------------

def _fade_v(t):
    return t * t * t * (t * (t * 6 - 15) + 10)

class VectorPerlinNoise:
    """Vectorized 2D Perlin noise using numpy arrays."""

    def __init__(self, seed=0):
        rng = np.random.RandomState(seed)
        self.perm = np.arange(256, dtype=np.int32)
        rng.shuffle(self.perm)
        self.perm = np.tile(self.perm, 2)
        angles = rng.uniform(0, 2 * np.pi, 256)
        self.grad_x = np.cos(angles)
        self.grad_y = np.sin(angles)

    def noise_2d(self, x_arr, y_arr):
        """Evaluate noise at arrays of (x, y) coordinates."""
        xi = np.floor(x_arr).astype(np.int32) & 255
        yi = np.floor(y_arr).astype(np.int32) & 255
        xf = x_arr - np.floor(x_arr)
        yf = y_arr - np.floor(y_arr)
        u = _fade_v(xf)
        v = _fade_v(yf)

        aa = self.perm[self.perm[xi] + yi]
        ab = self.perm[self.perm[xi] + yi + 1]
        ba = self.perm[self.perm[xi + 1] + yi]
        bb = self.perm[self.perm[xi + 1] + yi + 1]

        def grad_dot(h, dx, dy):
            return self.grad_x[h % 256] * dx + self.grad_y[h % 256] * dy

        x1 = (1 - u) * grad_dot(aa, xf, yf) + u * grad_dot(ba, xf - 1, yf)
        x2 = (1 - u) * grad_dot(ab, xf, yf - 1) + u * grad_dot(bb, xf - 1, yf - 1)
        return (1 - v) * x1 + v * x2

    def fbm_2d(self, x_arr, y_arr, octaves=4, lacunarity=2.0, gain=0.5):
        result = np.zeros_like(x_arr, dtype=np.float64)
        amp = 1.0
        freq = 1.0
        for _ in range(octaves):
            result += amp * self.noise_2d(x_arr * freq, y_arr * freq)
            amp *= gain
            freq *= lacunarity
        return result


def continuous_noise(h, w, scale, octaves=4, seed=0):
    """Vectorized noise field — no Python pixel loops."""
    pn = VectorPerlinNoise(seed)
    yy, xx = np.mgrid[0:h, 0:w]
    x_scaled = xx.astype(np.float64) / TILE * scale
    y_scaled = yy.astype(np.float64) / TILE * scale
    field = pn.fbm_2d(x_scaled, y_scaled, octaves)
    mn, mx = field.min(), field.max()
    if mx - mn > 1e-8:
        field = (field - mn) / (mx - mn)
    return field


def continuous_voronoi(h, w, density=0.8, seed=0):
    """Chunked Voronoi — vectorized distance computation."""
    rng = np.random.RandomState(seed)
    n_pts = max(4, int(ROWS * COLS * density))
    pts = rng.uniform(0, 1, (n_pts, 2)) * np.array([w, h])
    cell_vals = rng.uniform(0.2, 1.0, n_pts)

    yy, xx = np.mgrid[0:h, 0:w]
    coords = np.stack([xx.ravel(), yy.ravel()], axis=1).astype(np.float64)

    chunk = 8192
    cells_flat = np.zeros(h * w, dtype=np.float64)
    edge_flat = np.zeros(h * w, dtype=np.float64)

    for start in range(0, h * w, chunk):
        end = min(start + chunk, h * w)
        c = coords[start:end]
        dists = np.sqrt(((c[:, None, :] - pts[None, :, :]) ** 2).sum(axis=2))
        nearest = np.argmin(dists, axis=1)
        cells_flat[start:end] = cell_vals[nearest]
        part = np.partition(dists, 2, axis=1)[:, :2]
        part.sort(axis=1)
        edge_flat[start:end] = part[:, 1] - part[:, 0]

    cells = cells_flat.reshape(h, w)
    edges = edge_flat.reshape(h, w)
    mn, mx = edges.min(), edges.max()
    if mx - mn > 1e-8:
        edges = (edges - mn) / (mx - mn)
    return cells, edges


# ---------------------------------------------------------------------------
# Pixel masks
# ---------------------------------------------------------------------------

def make_floor_mask():
    """Upscale layout to pixel resolution."""
    return np.repeat(np.repeat(LAYOUT, TILE, axis=0), TILE, axis=1).astype(bool)


# ---------------------------------------------------------------------------
# Themes
# ---------------------------------------------------------------------------

THEMES = {
    'jungle': {
        'name': 'Jungle Temple',
        'floor_base': (82, 78, 68),
        'floor_accent': (58, 53, 42),
        'mortar': (42, 38, 28),
        'moss_colors': [(40, 82, 30), (58, 105, 40), (48, 70, 35)],
        'stain': (50, 45, 35),
        'wall_top': (55, 50, 40),
        'wall_face_lit': (65, 60, 48),
        'wall_face_dark': (25, 22, 15),
        'exterior': (22, 42, 18),
        'exterior_accent': (12, 28, 10),
        'exterior_detail': (30, 55, 22),
        'shadow_color': (8, 12, 5),
        'stone_density': 0.7,
        'moss_coverage': 0.35,
        'contrast': 35,
    },
    'ice': {
        'name': 'Frozen Caverns',
        'floor_base': (162, 178, 195),
        'floor_accent': (125, 148, 172),
        'mortar': (95, 115, 142),
        'moss_colors': [(175, 205, 225), (200, 225, 242), (155, 185, 210)],
        'stain': (105, 128, 155),
        'wall_top': (120, 135, 155),
        'wall_face_lit': (130, 148, 168),
        'wall_face_dark': (50, 65, 88),
        'exterior': (38, 52, 72),
        'exterior_accent': (22, 32, 52),
        'exterior_detail': (50, 68, 90),
        'shadow_color': (15, 22, 40),
        'stone_density': 0.6,
        'moss_coverage': 0.25,
        'contrast': 45,
    },
    'volcano': {
        'name': 'Infernal Depths',
        'floor_base': (52, 38, 32),
        'floor_accent': (35, 22, 18),
        'mortar': (25, 15, 10),
        'moss_colors': [(185, 62, 18), (225, 125, 28), (145, 42, 12)],
        'stain': (65, 28, 12),
        'wall_top': (38, 28, 22),
        'wall_face_lit': (50, 38, 30),
        'wall_face_dark': (18, 12, 8),
        'exterior': (18, 6, 4),
        'exterior_accent': (55, 12, 4),
        'exterior_detail': (80, 20, 8),
        'shadow_color': (5, 2, 1),
        'stone_density': 1.0,
        'moss_coverage': 0.18,
        'contrast': 40,
    },
}


# ---------------------------------------------------------------------------
# Floor stamping
# ---------------------------------------------------------------------------

def stamp_floors(canvas_arr, pack_dir):
    print("    Floor tiles...", end='', flush=True)
    floors = []
    for i in range(16):
        p = pack_dir / 'floors' / f'base_{i}.png'
        if p.exists():
            img = Image.open(p).convert('RGBA').resize((TILE, TILE))
            floors.append(np.array(img, dtype=np.float64))
    if not floors:
        print(" no tiles found")
        return
    rng = np.random.RandomState(42)
    for r in range(ROWS):
        for c in range(COLS):
            if LAYOUT[r, c] != 1:
                continue
            tile = floors[rng.randint(len(floors))]
            canvas_arr[r*TILE:(r+1)*TILE, c*TILE:(c+1)*TILE] = tile
    print(" done")


# ---------------------------------------------------------------------------
# Continuous overlays (all vectorized with numpy)
# ---------------------------------------------------------------------------

def render_overlays(canvas_arr, floor_mask, inner_dist, theme):
    for i, moss_color in enumerate(theme['moss_colors']):
        color = np.array(moss_color, dtype=np.float64)
        n = continuous_noise(PX_H, PX_W, scale=0.6 + i * 0.3, octaves=3, seed=200 + i * 37)
        detail = continuous_noise(PX_H, PX_W, scale=2.5 + i, octaves=2, seed=210 + i * 37)
        threshold = 1.0 - theme['moss_coverage']
        mask = np.clip((n - threshold) / (1.0 - threshold + 1e-8), 0, 1)
        mask *= (0.6 + 0.4 * detail)
        proximity = np.clip(1.0 - inner_dist / (TILE * 2.5), 0, 1) ** 0.5
        mask *= (0.3 + 0.7 * proximity)
        alpha = np.clip(mask * 0.7, 0, 1)
        alpha[~floor_mask] = 0
        for c in range(3):
            canvas_arr[:, :, c] += (color[c] - canvas_arr[:, :, c]) * alpha

    # Stains
    stain_color = np.array(theme['stain'], dtype=np.float64)
    stain_n = continuous_noise(PX_H, PX_W, scale=0.4, octaves=4, seed=300)
    stain_a = np.clip(stain_n * 1.8 - 0.6, 0, 1) ** 1.2 * 0.35
    stain_a[~floor_mask] = 0
    for c in range(3):
        canvas_arr[:, :, c] += (stain_color[c] - canvas_arr[:, :, c]) * stain_a

    # Cracks
    _, crack_edges = continuous_voronoi(PX_H, PX_W, density=1.5, seed=400)
    cracks = np.clip(1.0 - crack_edges * 5.0, 0, 1) ** 3
    crack_sparse = continuous_noise(PX_H, PX_W, scale=0.5, octaves=2, seed=401)
    cracks *= np.clip(crack_sparse * 2.5 - 1.0, 0, 1)
    crack_a = cracks * 0.4
    crack_a[~floor_mask] = 0
    mortar = np.array(theme['mortar'], dtype=np.float64) * 0.6
    for c in range(3):
        canvas_arr[:, :, c] += (mortar[c] - canvas_arr[:, :, c]) * crack_a


def render_ao(canvas_arr, floor_mask, inner_dist):
    ao = np.clip(1.0 - inner_dist / (TILE * 0.5), 0, 1) ** 1.8 * 0.45
    ao[~floor_mask] = 0
    for c in range(3):
        canvas_arr[:, :, c] *= (1 - ao)


def render_radial_lighting(canvas_arr, floor_mask, inner_dist, theme):
    """
    Center-radial lighting: each floor area is brightest at its center
    and darkens toward the walls. Uses the distance-from-wall field
    (inner_dist) inverted — pixels far from walls are bright (center),
    pixels near walls are darker.

    This replaces directional shadow casting with omnidirectional vignette.
    """
    # inner_dist = distance from nearest wall for each floor pixel
    # Normalize per-room by using a moderate radius
    max_radius = TILE * 2.5  # full brightness ~2.5 tiles from wall
    # Brightness: 1.0 at center, falls off toward walls
    brightness = np.clip(inner_dist / max_radius, 0, 1) ** 0.6
    brightness[~floor_mask] = 0

    # Invert to get shadow intensity (dark near walls, bright at center)
    shadow = (1.0 - brightness) * 0.35  # max 35% darkening at walls

    shadow_col = np.array(theme['shadow_color'], dtype=np.float64)
    for c in range(3):
        canvas_arr[:, :, c] = np.where(
            floor_mask,
            canvas_arr[:, :, c] * (1 - shadow) + shadow_col[c] * shadow,
            canvas_arr[:, :, c])


def render_exterior(canvas_arr, floor_mask, theme):
    """Exterior with Voronoi stone/terrain pattern for more visual interest."""
    # Voronoi cells for structure
    cells, edges = continuous_voronoi(PX_H, PX_W, density=0.4, seed=600)
    # Noise for organic variation
    n1 = continuous_noise(PX_H, PX_W, scale=0.8, octaves=4, seed=601)
    n2 = continuous_noise(PX_H, PX_W, scale=4.0, octaves=3, seed=602)

    base = np.array(theme['exterior'], dtype=np.float64)
    accent = np.array(theme['exterior_accent'], dtype=np.float64)
    detail = np.array(theme['exterior_detail'], dtype=np.float64)
    void_mask = ~floor_mask

    # Base color from cell values
    for c in range(3):
        cell_color = base[c] * cells + accent[c] * (1 - cells)
        # Add noise variation
        variation = (n1 - 0.5) * 25 + (n2 - 0.5) * 12
        # Darken Voronoi edges (mortar/crevice lines)
        edge_darken = np.clip(1.0 - edges * 3.0, 0, 1) ** 1.5
        color = cell_color + variation
        # Blend in detail color at edges
        color = color * (1 - edge_darken * 0.5) + detail[c] * 0.3 * edge_darken
        canvas_arr[:, :, c] = np.where(void_mask, np.clip(color, 0, 255), canvas_arr[:, :, c])
    canvas_arr[:, :, 3] = 255


# ---------------------------------------------------------------------------
# Thin wall borders + 2.5D faces (vectorized)
# ---------------------------------------------------------------------------

def render_walls(canvas_arr, floor_mask, theme):
    """
    2.5D wall rendering — looking DOWN into rooms.

    Far walls (north, west) show their inner face INTO the floor area.
    Near walls (south, east) show only the wall-top border.
    This creates the illusion of recessed rooms, not elevated platforms.

    - Wall top border: floor pixels within WALL_THICKNESS of any edge
    - North inner face: floor pixels below a north edge, painted as
      the far wall face receding downward (darker toward floor)
    - West inner face: floor pixels right of a west edge, painted as
      the far wall face receding rightward
    - South/east edges: bright highlight (wall top catch light)
    """
    inner_dist = distance_transform_edt(floor_mask)
    wall_noise = continuous_noise(PX_H, PX_W, scale=2.0, octaves=2,
                                  seed=hash(theme['name']) & 0xFFFF)

    wall_top = np.array(theme['wall_top'], dtype=np.float64)
    face_lit = np.array(theme['wall_face_lit'], dtype=np.float64)
    face_dark = np.array(theme['wall_face_dark'], dtype=np.float64)

    # --- Voronoi stone texture for wall faces ---
    # Generate a separate stone pattern for the walls so faces have block detail
    print("    Wall stone texture...", end='', flush=True)
    wall_cells, wall_edges = continuous_voronoi(PX_H, PX_W, density=1.2, seed=700)
    wall_stone_var = (wall_cells - 0.5) * 18  # per-block color variation
    wall_mortar = np.clip(1.0 - wall_edges * 4.5, 0, 1) ** 1.5  # mortar lines
    print(" done")

    # --- Wall top border (all edges) ---
    # Flat stone surface, no gradient. Textured with noise + stone blocks.
    print("    Wall borders...", end='', flush=True)
    border_mask = floor_mask & (inner_dist <= WALL_THICKNESS)
    noise_var = (wall_noise - 0.5) * 10
    for c in range(3):
        # Flat color with noise texture, no brightness gradient
        base_val = wall_top[c] + noise_var + wall_stone_var
        # Darken mortar lines
        base_val = base_val * (1 - wall_mortar * 0.4) + (wall_top[c] * 0.5) * wall_mortar * 0.4
        canvas_arr[:, :, c] = np.where(
            border_mask, np.clip(base_val, 0, 255), canvas_arr[:, :, c])
    print(" done")

    # --- Dark edge line between wall top and inner face (all 4 edges) ---
    print("    Edge creases...", end='', flush=True)
    north_edge = np.zeros((PX_H, PX_W), dtype=bool)
    north_edge[1:, :] = floor_mask[1:, :] & ~floor_mask[:-1, :]
    west_edge = np.zeros((PX_H, PX_W), dtype=bool)
    west_edge[:, 1:] = floor_mask[:, 1:] & ~floor_mask[:, :-1]
    south_edge_c = np.zeros((PX_H, PX_W), dtype=bool)
    south_edge_c[:-1, :] = floor_mask[:-1, :] & ~floor_mask[1:, :]
    east_edge_c = np.zeros((PX_H, PX_W), dtype=bool)
    east_edge_c[:, :-1] = floor_mask[:, :-1] & ~floor_mask[:, 1:]

    crease_color = face_dark * 0.7
    for d in range(2):
        a = 0.6 * (1 - d * 0.4)
        offset = WALL_THICKNESS + d
        # North crease (shift down)
        crease = np.zeros((PX_H, PX_W), dtype=bool)
        if offset < PX_H:
            crease[offset:, :] = north_edge[:-offset, :]
        crease &= floor_mask
        for c in range(3):
            canvas_arr[:, :, c] = np.where(crease,
                np.clip(canvas_arr[:, :, c] * (1 - a) + crease_color[c] * a, 0, 255),
                canvas_arr[:, :, c])
        # West crease (shift right)
        crease = np.zeros((PX_H, PX_W), dtype=bool)
        if offset < PX_W:
            crease[:, offset:] = west_edge[:, :-offset]
        crease &= floor_mask
        for c in range(3):
            canvas_arr[:, :, c] = np.where(crease,
                np.clip(canvas_arr[:, :, c] * (1 - a) + crease_color[c] * a, 0, 255),
                canvas_arr[:, :, c])
        # South crease (shift up)
        crease = np.zeros((PX_H, PX_W), dtype=bool)
        if offset < PX_H:
            crease[:-offset, :] = south_edge_c[offset:, :]
        crease &= floor_mask
        for c in range(3):
            canvas_arr[:, :, c] = np.where(crease,
                np.clip(canvas_arr[:, :, c] * (1 - a) + crease_color[c] * a, 0, 255),
                canvas_arr[:, :, c])
        # East crease (shift left)
        crease = np.zeros((PX_H, PX_W), dtype=bool)
        if offset < PX_W:
            crease[:, :-offset] = east_edge_c[:, offset:]
        crease &= floor_mask
        for c in range(3):
            canvas_arr[:, :, c] = np.where(crease,
                np.clip(canvas_arr[:, :, c] * (1 - a) + crease_color[c] * a, 0, 255),
                canvas_arr[:, :, c])
    print(" done")

    # --- All four inner faces (uniform lighting, radial handles depth) ---
    print("    Inner faces...", end='', flush=True)
    # South edge (void below) and East edge (void right) also get faces now
    south_edge = np.zeros((PX_H, PX_W), dtype=bool)
    south_edge[:-1, :] = floor_mask[:-1, :] & ~floor_mask[1:, :]
    east_edge = np.zeros((PX_H, PX_W), dtype=bool)
    east_edge[:, :-1] = floor_mask[:, :-1] & ~floor_mask[:, 1:]

    edges = [
        ('n', north_edge, 'y', 1),   # shift down into floor
        ('w', west_edge,  'x', 1),   # shift right into floor
        ('s', south_edge, 'y', -1),  # shift up into floor
        ('e', east_edge,  'x', -1),  # shift left into floor
    ]

    for name, edge, axis, direction in edges:
        for d in range(WALL_FACE_HEIGHT):
            target = np.zeros((PX_H, PX_W), dtype=bool)
            offset = WALL_THICKNESS + 2 + d
            if axis == 'y':
                if direction > 0 and offset < PX_H:
                    target[offset:, :] = edge[:-offset, :]
                elif direction < 0 and offset < PX_H:
                    target[:-offset, :] = edge[offset:, :]
            else:
                if direction > 0 and offset < PX_W:
                    target[:, offset:] = edge[:, :-offset]
                elif direction < 0 and offset < PX_W:
                    target[:, :-offset] = edge[:, offset:]
            paint_mask = target & floor_mask
            t = (d / WALL_FACE_HEIGHT) ** 0.7
            base = face_lit * (1 - t) + face_dark * t
            for c in range(3):
                color = base[c] + np.where(paint_mask, wall_stone_var * (1 - t * 0.5), 0)
                color = color * (1 - wall_mortar * 0.3 * (1 - t)) + face_dark[c] * wall_mortar * 0.3 * (1 - t)
                color += np.where(paint_mask, (wall_noise - 0.5) * 8 * (1 - t), 0)
                canvas_arr[:, :, c] = np.where(
                    paint_mask, np.clip(color, 0, 255), canvas_arr[:, :, c])
    print(" done")


# ---------------------------------------------------------------------------
# Grid
# ---------------------------------------------------------------------------

def render_grid(canvas, floor_mask_arr):
    draw = ImageDraw.Draw(canvas)
    for r in range(ROWS):
        for c in range(COLS):
            if LAYOUT[r, c] != 1:
                continue
            x, y = c * TILE, r * TILE
            draw.line([(x, y + TILE - 1), (x + TILE - 1, y + TILE - 1)],
                      fill=(0, 0, 0, 20), width=1)
            draw.line([(x + TILE - 1, y), (x + TILE - 1, y + TILE - 1)],
                      fill=(0, 0, 0, 20), width=1)


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def render_preview(theme_name, pack_dir, output_path):
    theme = THEMES[theme_name]
    print(f"\n  {theme['name']}:")

    canvas_arr = np.zeros((PX_H, PX_W, 4), dtype=np.float64)
    canvas_arr[:, :, 3] = 255
    floor_mask = make_floor_mask()
    inner_dist = distance_transform_edt(floor_mask)

    print("    Exterior...", end='', flush=True)
    render_exterior(canvas_arr, floor_mask, theme)
    print(" done")

    stamp_floors(canvas_arr, pack_dir)

    print("    Overlays...", end='', flush=True)
    render_overlays(canvas_arr, floor_mask, inner_dist, theme)
    print(" done")

    render_ao(canvas_arr, floor_mask, inner_dist)
    render_radial_lighting(canvas_arr, floor_mask, inner_dist, theme)
    render_walls(canvas_arr, floor_mask, theme)

    canvas = Image.fromarray(np.clip(canvas_arr, 0, 255).astype(np.uint8), 'RGBA')
    render_grid(canvas, floor_mask)
    canvas.save(output_path)
    print(f"  -> {output_path}")
    return canvas


def backup_previous(preview_dir, themes):
    """Save existing previews as 'previous_*' for A/B comparison."""
    backed_up = False
    for name in themes + ['comparison']:
        src = preview_dir / f'{name}_preview.png' if name != 'comparison' else preview_dir / 'comparison.png'
        dst = preview_dir / f'previous_{name}_preview.png' if name != 'comparison' else preview_dir / 'previous_comparison.png'
        if src.exists():
            import shutil
            shutil.copy2(src, dst)
            backed_up = True
    if backed_up:
        print("  Backed up previous previews as previous_*")


def main():
    import time
    t0 = time.time()
    script_dir = Path(__file__).resolve().parent
    packs_dir = script_dir.parent.parent / 'assets' / 'packs'
    preview_dir = packs_dir.parent / 'previews'
    preview_dir.mkdir(parents=True, exist_ok=True)

    themes = ['jungle', 'ice', 'volcano']

    # Always backup previous renders before overwriting
    backup_previous(preview_dir, themes)

    previews = []
    for name in themes:
        pack = packs_dir / name
        img = render_preview(name, pack, preview_dir / f'{name}_preview.png')
        previews.append(img)

    if previews:
        gap = 8
        total_w = sum(p.width for p in previews) + (len(previews) - 1) * gap
        max_h = max(p.height for p in previews)
        comp = Image.new('RGBA', (total_w, max_h), (20, 20, 20, 255))
        x = 0
        for p in previews:
            comp.paste(p, (x, 0))
            x += p.width + gap
        comp.save(preview_dir / 'comparison.png')
        print(f"\nComparison: {preview_dir / 'comparison.png'}")

    print(f"Total time: {time.time() - t0:.1f}s")


if __name__ == '__main__':
    main()
