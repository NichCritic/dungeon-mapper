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
        'floor_base': (155, 142, 118),       # Warm tan sandstone
        'floor_accent': (135, 122, 98),      # Slightly darker tan
        'mortar': (65, 58, 42),              # Dark grout
        'moss_colors': [(45, 85, 32), (62, 110, 42), (30, 55, 22)],
        'stain': (90, 82, 60),              # Warm dirt stain
        'wall_top': (75, 68, 52),
        'wall_face_lit': (85, 78, 60),
        'wall_face_dark': (30, 25, 18),
        'exterior': (12, 25, 10),
        'exterior_accent': (6, 14, 5),
        'exterior_detail': (18, 35, 12),
        'shadow_color': (20, 22, 15),
        'stone_density': 0.7,
        'moss_coverage': 0.45,
        'contrast': 15,                      # Subtle tile variation
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
        'exterior': (18, 28, 42),
        'exterior_accent': (10, 16, 28),
        'exterior_detail': (25, 38, 55),
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
        'exterior': (12, 4, 2),
        'exterior_accent': (30, 8, 3),
        'exterior_detail': (45, 12, 5),
        'shadow_color': (5, 2, 1),
        'stone_density': 1.0,
        'moss_coverage': 0.18,
        'contrast': 40,
    },
}


# ---------------------------------------------------------------------------
# Continuous floor rendering (no tile stamping)
# ---------------------------------------------------------------------------

def generate_floor_tile(size, theme, seed):
    """
    Generate a simple, subtle floor tile. Mostly flat with gentle
    color variation — the detail comes from grout and overlays,
    not the tile surface itself.
    """
    pn = VectorPerlinNoise(seed)
    rng = np.random.RandomState(seed)

    base = np.array(theme['floor_base'], dtype=np.float64)
    accent = np.array(theme['floor_accent'], dtype=np.float64)

    yy, xx = np.mgrid[0:size, 0:size]
    # Gentle surface variation (low frequency, low amplitude)
    x_scaled = xx.astype(np.float64) / size * 3.0
    y_scaled = yy.astype(np.float64) / size * 3.0
    surface = pn.fbm_2d(x_scaled, y_scaled, octaves=2)
    surface = (surface - surface.min()) / (surface.max() - surface.min() + 1e-8)

    # Very fine grain texture
    x_fine = xx.astype(np.float64) / size * 10.0
    y_fine = yy.astype(np.float64) / size * 10.0
    fine = pn.fbm_2d(x_fine + 50, y_fine + 50, octaves=2)
    fine = (fine - fine.min()) / (fine.max() - fine.min() + 1e-8)

    # Random overall hue shift for this tile
    hue_shift = rng.uniform(-0.15, 0.15)

    tile = np.zeros((size, size, 4), dtype=np.float64)
    t = 0.5 + hue_shift + (surface - 0.5) * 0.3
    t = np.clip(t, 0, 1)
    for c in range(3):
        color = base[c] * t + accent[c] * (1 - t)
        # Very subtle fine grain
        color += (fine - 0.5) * 8
        tile[:, :, c] = np.clip(color, 0, 255)
    tile[:, :, 3] = 255
    return tile


def stamp_unique_floors(canvas_arr, floor_mask, theme):
    """Stamp a unique procedural tile for every floor cell. No repeats."""
    print("    Floor tiles...", end='', flush=True)
    floor_cells = [(r, c) for r in range(ROWS) for c in range(COLS) if LAYOUT[r, c] == 1]
    n_tiles = max(len(floor_cells), 128)
    tiles = [generate_floor_tile(TILE, theme, seed=1000 + i) for i in range(n_tiles)]

    rng = np.random.RandomState(42)
    indices = list(range(n_tiles))
    rng.shuffle(indices)
    for idx, (r, c) in enumerate(floor_cells):
        tile = tiles[indices[idx % n_tiles]]
        canvas_arr[r*TILE:(r+1)*TILE, c*TILE:(c+1)*TILE] = tile
    print(" done")


def render_grout(canvas_arr, floor_mask, theme):
    """
    Draw grout/mortar lines between floor tiles with varying width
    and darkness. Each edge gets a slightly different width (1-3px)
    seeded by position. Some edges get moss-filled grout.
    """
    print("    Grout lines...", end='', flush=True)
    mortar = np.array(theme['mortar'], dtype=np.float64)
    moss_colors = theme['moss_colors']
    rng = np.random.RandomState(99)

    for r in range(ROWS):
        for c in range(COLS):
            if LAYOUT[r, c] != 1:
                continue
            x0, y0 = c * TILE, r * TILE

            # Determine grout properties seeded by position
            h_seed = hash((r, c, 'h')) & 0xFFFF
            v_seed = hash((r, c, 'v')) & 0xFFFF
            h_rng = np.random.RandomState(h_seed)
            v_rng = np.random.RandomState(v_seed)

            # Bottom edge grout (if cell below is also floor)
            h_width = h_rng.randint(1, 4)  # 1-3px
            h_dark = 0.5 + h_rng.uniform(0, 0.4)  # darkness
            h_mossy = h_rng.random() < theme['moss_coverage']

            for dy in range(h_width):
                py = y0 + TILE - 1 - dy
                if py < 0 or py >= PX_H:
                    continue
                for px in range(x0, min(x0 + TILE, PX_W)):
                    if not floor_mask[py, px]:
                        continue
                    t = h_dark * (1 - dy / max(h_width, 1) * 0.3)
                    if h_mossy and len(moss_colors) > 0:
                        mc = np.array(moss_colors[0], dtype=np.float64)
                        for ch in range(3):
                            canvas_arr[py, px, ch] = np.clip(
                                canvas_arr[py, px, ch] * (1 - t * 0.5) + mc[ch] * t * 0.5, 0, 255)
                    else:
                        for ch in range(3):
                            canvas_arr[py, px, ch] = np.clip(
                                canvas_arr[py, px, ch] * (1 - t) + mortar[ch] * t, 0, 255)

            # Right edge grout
            v_width = v_rng.randint(1, 4)
            v_dark = 0.5 + v_rng.uniform(0, 0.4)
            v_mossy = v_rng.random() < theme['moss_coverage']

            for dx in range(v_width):
                px = x0 + TILE - 1 - dx
                if px < 0 or px >= PX_W:
                    continue
                for py in range(y0, min(y0 + TILE, PX_H)):
                    if not floor_mask[py, px]:
                        continue
                    t = v_dark * (1 - dx / max(v_width, 1) * 0.3)
                    if v_mossy and len(moss_colors) > 0:
                        mc = np.array(moss_colors[0], dtype=np.float64)
                        for ch in range(3):
                            canvas_arr[py, px, ch] = np.clip(
                                canvas_arr[py, px, ch] * (1 - t * 0.5) + mc[ch] * t * 0.5, 0, 255)
                    else:
                        for ch in range(3):
                            canvas_arr[py, px, ch] = np.clip(
                                canvas_arr[py, px, ch] * (1 - t) + mortar[ch] * t, 0, 255)
    print(" done")


def render_vine_invasion(canvas_arr, floor_mask, inner_dist, theme):
    """
    Vines/foliage/frost/lava creeping from the exterior into the rooms
    along the walls. Uses wall proximity + noise to create organic
    tendrils that invade the floor space.
    """
    if theme['moss_coverage'] <= 0:
        return
    print("    Vine invasion...", end='', flush=True)
    # Multiple noise scales for organic tendril shapes
    n1 = continuous_noise(PX_H, PX_W, scale=1.5, octaves=4, seed=800)
    n2 = continuous_noise(PX_H, PX_W, scale=4.0, octaves=3, seed=801)
    n3 = continuous_noise(PX_H, PX_W, scale=0.5, octaves=2, seed=802)

    # Invasion distance: how far vines reach from wall
    max_invasion = TILE * 1.8
    # Base invasion mask: strong near walls, fading inward
    wall_prox = np.clip(1.0 - inner_dist / max_invasion, 0, 1)

    # Shape the invasion with noise — creates tendril/finger shapes
    tendril = wall_prox * (0.3 + 0.7 * n1) * (0.5 + 0.5 * n2)
    # Large-scale variation: some areas have more invasion
    tendril *= (0.2 + 0.8 * np.clip(n3 * 1.5, 0, 1))

    # Threshold to get defined edges
    vine_mask = np.clip((tendril - 0.25) * 4.0, 0, 1)
    vine_mask[~floor_mask] = 0

    # Use multiple moss colors for depth
    colors = [np.array(mc, dtype=np.float64) for mc in theme['moss_colors']]
    if not colors:
        return

    # Layer 1: base vine color (darkest)
    alpha1 = vine_mask * 0.6
    for c in range(3):
        canvas_arr[:, :, c] += (colors[0][c] - canvas_arr[:, :, c]) * alpha1

    # Layer 2: highlights on thicker parts
    if len(colors) > 1:
        highlight_mask = np.clip((tendril - 0.4) * 5.0, 0, 1) * vine_mask
        alpha2 = highlight_mask * 0.4
        for c in range(3):
            canvas_arr[:, :, c] += (colors[1][c] - canvas_arr[:, :, c]) * alpha2

    # Layer 3: dark edges/shadows on vine mass
    if len(colors) > 2:
        # Use the gradient at the edge of vine coverage for shadow
        edge_dark = vine_mask * np.clip(1.0 - tendril * 2, 0, 1)
        alpha3 = edge_dark * 0.3
        dark = colors[2] * 0.5
        for c in range(3):
            canvas_arr[:, :, c] += (dark[c] - canvas_arr[:, :, c]) * alpha3

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
    """
    Exterior with dense, fine-grained texture.
    Multiple Voronoi scales for terrain-like complexity.
    Higher contrast and more detail than before.
    """
    # Small-scale Voronoi for dense terrain texture (rocks, foliage, etc.)
    cells_sm, edges_sm = continuous_voronoi(PX_H, PX_W, density=2.0, seed=600)
    # Medium-scale for larger features
    cells_md, edges_md = continuous_voronoi(PX_H, PX_W, density=0.5, seed=601)
    # Multi-scale noise
    n_coarse = continuous_noise(PX_H, PX_W, scale=0.6, octaves=4, seed=602)
    n_fine = continuous_noise(PX_H, PX_W, scale=3.0, octaves=3, seed=603)
    n_micro = continuous_noise(PX_H, PX_W, scale=8.0, octaves=2, seed=604)

    base = np.array(theme['exterior'], dtype=np.float64)
    accent = np.array(theme['exterior_accent'], dtype=np.float64)
    detail = np.array(theme['exterior_detail'], dtype=np.float64)
    void_mask = ~floor_mask

    for c in range(3):
        # Blend base/accent at medium scale
        color = base[c] * (0.4 + 0.6 * cells_md) + accent[c] * (0.6 - 0.6 * cells_md)
        # Small-scale cell variation (the fine texture)
        color += (cells_sm - 0.5) * 20
        # Multi-scale noise
        color += (n_coarse - 0.5) * 18
        color += (n_fine - 0.5) * 12
        color += (n_micro - 0.5) * 6
        # Small Voronoi edges as dark crevice lines
        edge_dark_sm = np.clip(1.0 - edges_sm * 5.0, 0, 1) ** 2.0
        color = color * (1 - edge_dark_sm * 0.4) + accent[c] * 0.5 * edge_dark_sm
        # Medium Voronoi edges as broader dark lines
        edge_dark_md = np.clip(1.0 - edges_md * 3.0, 0, 1) ** 1.5
        color = color * (1 - edge_dark_md * 0.3) + detail[c] * 0.4 * edge_dark_md
        canvas_arr[:, :, c] = np.where(void_mask, np.clip(color, 0, 255), canvas_arr[:, :, c])
    canvas_arr[:, :, 3] = 255


# ---------------------------------------------------------------------------
# Thin wall borders + 2.5D faces (vectorized)
# ---------------------------------------------------------------------------

def render_walls(canvas_arr, floor_mask, theme):
    """
    2.5D wall rendering — walls are OUTSIDE the floor extent.

    Wall top border and inner faces are drawn in the void, adjacent
    to floor edges. The floor area stays full-size with no wall
    intrusion. Inner faces extend from the wall top inward toward
    the floor, creating a ledge/cliff effect seen from above.
    """
    void_mask = ~floor_mask
    # Distance from floor edge into the void
    void_dist = distance_transform_edt(void_mask)
    wall_noise = continuous_noise(PX_H, PX_W, scale=2.0, octaves=2,
                                  seed=hash(theme['name']) & 0xFFFF)

    wall_top = np.array(theme['wall_top'], dtype=np.float64)
    face_lit = np.array(theme['wall_face_lit'], dtype=np.float64)
    face_dark = np.array(theme['wall_face_dark'], dtype=np.float64)

    # --- Voronoi stone texture for walls (higher density, more contrast) ---
    print("    Wall stone texture...", end='', flush=True)
    wall_cells, wall_edges = continuous_voronoi(PX_H, PX_W, density=2.0, seed=700)
    wall_cells2, wall_edges2 = continuous_voronoi(PX_H, PX_W, density=0.6, seed=701)
    wall_stone_var = (wall_cells - 0.5) * 22 + (wall_cells2 - 0.5) * 10
    wall_mortar = np.clip(1.0 - wall_edges * 4.0, 0, 1) ** 1.5
    wall_mortar2 = np.clip(1.0 - wall_edges2 * 3.0, 0, 1) ** 1.5
    wall_mortar = np.maximum(wall_mortar, wall_mortar2 * 0.6)  # combine both scales
    print(" done")

    # Total wall extent into void = border + face
    total_wall = WALL_THICKNESS + WALL_FACE_HEIGHT

    # --- Inner faces first (furthest from floor, drawn behind wall top) ---
    # These are void pixels near the floor edge, beyond the wall top border.
    # Face goes from WALL_THICKNESS to WALL_THICKNESS + WALL_FACE_HEIGHT
    # away from the floor edge, getting darker as it recedes.
    print("    Inner faces...", end='', flush=True)

    # Find floor edges (void pixels adjacent to floor)
    # void_dist == 1 means immediately adjacent to floor
    # Wall top occupies void_dist 1..WALL_THICKNESS
    # Face occupies void_dist WALL_THICKNESS+1..total_wall

    for d in range(WALL_FACE_HEIGHT):
        dist_val = WALL_THICKNESS + 1 + d
        face_band = void_mask & (void_dist >= dist_val - 0.5) & (void_dist < dist_val + 0.5)
        # t=0 at top of face (near wall border), t=1 at bottom (deep)
        t = (d / WALL_FACE_HEIGHT) ** 0.7
        base = face_lit * (1 - t) + face_dark * t
        for c in range(3):
            color = base[c] + np.where(face_band, wall_stone_var * (1 - t * 0.5), 0)
            color = color * (1 - wall_mortar * 0.3 * (1 - t)) + face_dark[c] * wall_mortar * 0.3 * (1 - t)
            color += np.where(face_band, (wall_noise - 0.5) * 8 * (1 - t), 0)
            canvas_arr[:, :, c] = np.where(
                face_band, np.clip(color, 0, 255), canvas_arr[:, :, c])
    print(" done")

    # --- Dark crease line between wall top and inner face ---
    print("    Edge creases...", end='', flush=True)
    crease_color = face_dark * 0.7
    for d in range(2):
        a = 0.6 * (1 - d * 0.4)
        dist_val = WALL_THICKNESS + d
        crease = void_mask & (void_dist >= dist_val + 0.5) & (void_dist < dist_val + 1.5)
        for c in range(3):
            canvas_arr[:, :, c] = np.where(crease,
                np.clip(canvas_arr[:, :, c] * (1 - a) + crease_color[c] * a, 0, 255),
                canvas_arr[:, :, c])
    print(" done")

    # --- Wall top border (void pixels closest to floor edge) ---
    print("    Wall borders...", end='', flush=True)
    border_mask = void_mask & (void_dist <= WALL_THICKNESS)
    noise_var = (wall_noise - 0.5) * 10
    for c in range(3):
        base_val = wall_top[c] + noise_var + wall_stone_var
        base_val = base_val * (1 - wall_mortar * 0.4) + (wall_top[c] * 0.5) * wall_mortar * 0.4
        canvas_arr[:, :, c] = np.where(
            border_mask, np.clip(base_val, 0, 255), canvas_arr[:, :, c])
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

def render_preview(theme_name, output_path):
    theme = THEMES[theme_name]
    print(f"\n  {theme['name']}:")

    canvas_arr = np.zeros((PX_H, PX_W, 4), dtype=np.float64)
    canvas_arr[:, :, 3] = 255
    floor_mask = make_floor_mask()
    inner_dist = distance_transform_edt(floor_mask)

    print("    Exterior...", end='', flush=True)
    render_exterior(canvas_arr, floor_mask, theme)
    print(" done")

    stamp_unique_floors(canvas_arr, floor_mask, theme)
    render_grout(canvas_arr, floor_mask, theme)

    print("    Overlays...", end='', flush=True)
    render_overlays(canvas_arr, floor_mask, inner_dist, theme)
    print(" done")

    render_vine_invasion(canvas_arr, floor_mask, inner_dist, theme)
    render_ao(canvas_arr, floor_mask, inner_dist)
    render_radial_lighting(canvas_arr, floor_mask, inner_dist, theme)
    render_walls(canvas_arr, floor_mask, theme)

    canvas = Image.fromarray(np.clip(canvas_arr, 0, 255).astype(np.uint8), 'RGBA')
    # No more grid overlay — grout lines replace it
    # render_grid(canvas, floor_mask)
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
        img = render_preview(name, preview_dir / f'{name}_preview.png')
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
