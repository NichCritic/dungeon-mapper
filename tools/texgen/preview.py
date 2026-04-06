#!/usr/bin/env python3
"""
Compose preview images with continuous (non-tiling) texture generation.

Instead of stamping pre-made tiles, this generates floor/wall textures as
continuous fields across the entire map, then masks them with the geometry.
This eliminates visible tile boundaries and allows features (cracks, moss,
stains) to flow naturally across cell boundaries.

2.5D walls with south/east faces, map-aware shadow casting.
"""

import json
import sys
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFilter

# ---------------------------------------------------------------------------
# Layout
# ---------------------------------------------------------------------------

TILE = 64

# 0=void, 1=floor, 2=wall
LAYOUT = [
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0, 2, 2, 2, 2, 2, 2, 0, 0, 0, 0, 0, 0, 0],
    [0, 2, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0],
    [0, 2, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0],
    [0, 2, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 0],
    [0, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 0],
    [0, 2, 1, 1, 1, 1, 2, 2, 2, 1, 1, 1, 2, 0],
    [0, 2, 2, 2, 1, 2, 2, 0, 2, 1, 1, 1, 2, 0],
    [0, 0, 0, 2, 1, 2, 0, 0, 2, 1, 1, 1, 2, 0],
    [0, 0, 0, 2, 1, 2, 0, 0, 2, 2, 2, 2, 2, 0],
    [0, 0, 0, 2, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0],
    [0, 2, 2, 2, 1, 2, 2, 2, 0, 0, 0, 0, 0, 0],
    [0, 2, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0],
    [0, 2, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0],
    [0, 2, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0],
    [0, 2, 2, 2, 2, 2, 2, 2, 0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
]

ROWS = len(LAYOUT)
COLS = len(LAYOUT[0])
PX_H = ROWS * TILE
PX_W = COLS * TILE


def cell(r, c):
    if 0 <= r < ROWS and 0 <= c < COLS:
        return LAYOUT[r][c]
    return 0

def is_floor(r, c): return cell(r, c) == 1
def is_wall(r, c): return cell(r, c) == 2
def is_void(r, c): return cell(r, c) == 0
def is_solid(r, c): return cell(r, c) != 1


# ---------------------------------------------------------------------------
# Noise (continuous, map-scale)
# ---------------------------------------------------------------------------

def _fade(t):
    return t * t * t * (t * (t * 6 - 15) + 10)

def _lerp(a, b, t):
    return a + t * (b - a)

class PerlinNoise:
    def __init__(self, seed=0):
        rng = np.random.RandomState(seed)
        self.perm = np.arange(256, dtype=int)
        rng.shuffle(self.perm)
        self.perm = np.tile(self.perm, 2)
        angles = rng.uniform(0, 2 * np.pi, 256)
        self.grads = np.stack([np.cos(angles), np.sin(angles)], axis=1)

    def _grad(self, h, x, y):
        g = self.grads[h % 256]
        return g[0] * x + g[1] * y

    def noise(self, x, y):
        xi = int(np.floor(x)) & 255
        yi = int(np.floor(y)) & 255
        xf = x - np.floor(x)
        yf = y - np.floor(y)
        u = _fade(xf)
        v = _fade(yf)
        aa = self.perm[self.perm[xi] + yi]
        ab = self.perm[self.perm[xi] + yi + 1]
        ba = self.perm[self.perm[xi + 1] + yi]
        bb = self.perm[self.perm[xi + 1] + yi + 1]
        x1 = _lerp(self._grad(aa, xf, yf), self._grad(ba, xf - 1, yf), u)
        x2 = _lerp(self._grad(ab, xf, yf - 1), self._grad(bb, xf - 1, yf - 1), u)
        return _lerp(x1, x2, v)

    def fbm(self, x, y, octaves=4, lacunarity=2.0, gain=0.5):
        val = 0.0
        amp = 1.0
        freq = 1.0
        for _ in range(octaves):
            val += amp * self.noise(x * freq, y * freq)
            amp *= gain
            freq *= lacunarity
        return val


def continuous_noise(h, w, scale, octaves=4, seed=0):
    """Full-resolution noise field across the map. No tiling artifacts."""
    pn = PerlinNoise(seed)
    field = np.zeros((h, w), dtype=np.float64)
    for y in range(h):
        for x in range(w):
            field[y, x] = pn.fbm(x / TILE * scale, y / TILE * scale, octaves)
    mn, mx = field.min(), field.max()
    if mx - mn > 1e-8:
        field = (field - mn) / (mx - mn)
    return field


def continuous_voronoi(h, w, density=0.8, seed=0):
    """
    Map-scale Voronoi pattern. density = points per tile.
    Returns (cell_values, edge_field) both [0,1].
    """
    rng = np.random.RandomState(seed)
    n_pts = int(ROWS * COLS * density)
    pts = rng.uniform(0, 1, (n_pts, 2)) * np.array([w, h])
    cell_vals = rng.uniform(0.2, 1.0, n_pts)

    yy, xx = np.mgrid[0:h, 0:w]
    coords = np.stack([xx.ravel(), yy.ravel()], axis=1).astype(np.float64)

    # Chunked distance computation to avoid massive memory use
    chunk = 4096
    cells_flat = np.zeros(h * w, dtype=np.float64)
    d1_flat = np.zeros(h * w, dtype=np.float64)
    d2_flat = np.zeros(h * w, dtype=np.float64)

    for start in range(0, h * w, chunk):
        end = min(start + chunk, h * w)
        c = coords[start:end]
        dists = np.sqrt(((c[:, None, :] - pts[None, :, :]) ** 2).sum(axis=2))
        nearest = np.argmin(dists, axis=1)
        cells_flat[start:end] = cell_vals[nearest]
        sorted_d = np.partition(dists, 2, axis=1)[:, :2]
        sorted_d.sort(axis=1)
        d1_flat[start:end] = sorted_d[:, 0]
        d2_flat[start:end] = sorted_d[:, 1]

    cells = cells_flat.reshape(h, w)
    edges = (d2_flat - d1_flat).reshape(h, w)
    mn, mx = edges.min(), edges.max()
    if mx - mn > 1e-8:
        edges = (edges - mn) / (mx - mn)
    return cells, edges


# ---------------------------------------------------------------------------
# Geometry masks (pixel-level)
# ---------------------------------------------------------------------------

def make_floor_mask():
    """Boolean mask: True where layout cell is floor."""
    mask = np.zeros((PX_H, PX_W), dtype=bool)
    for r in range(ROWS):
        for c in range(COLS):
            if is_floor(r, c):
                mask[r*TILE:(r+1)*TILE, c*TILE:(c+1)*TILE] = True
    return mask


def make_wall_mask():
    mask = np.zeros((PX_H, PX_W), dtype=bool)
    for r in range(ROWS):
        for c in range(COLS):
            if is_wall(r, c):
                mask[r*TILE:(r+1)*TILE, c*TILE:(c+1)*TILE] = True
    return mask


def make_void_mask():
    mask = np.zeros((PX_H, PX_W), dtype=bool)
    for r in range(ROWS):
        for c in range(COLS):
            if is_void(r, c):
                mask[r*TILE:(r+1)*TILE, c*TILE:(c+1)*TILE] = True
    return mask


def wall_distance_field(floor_mask):
    """
    For each floor pixel, distance to nearest non-floor pixel.
    Used for AO and edge effects. Computed via simple BFS-like expansion.
    """
    from scipy.ndimage import distance_transform_edt
    dist = distance_transform_edt(floor_mask)
    return dist


# ---------------------------------------------------------------------------
# Theme palettes
# ---------------------------------------------------------------------------

THEMES = {
    'jungle': {
        'name': 'Jungle Temple',
        'floor_base': (82, 78, 68),
        'floor_accent': (58, 53, 42),
        'mortar': (42, 38, 28),
        'moss_colors': [(40, 82, 30), (58, 105, 40), (48, 70, 35)],
        'stain': (50, 45, 35),
        'wall_top': (100, 95, 82),
        'wall_face_lit': (78, 72, 58),
        'wall_face_dark': (32, 28, 20),
        'wall_mortar': (42, 38, 28),
        'exterior': (22, 42, 18),
        'exterior_accent': (12, 28, 10),
        'exterior_detail': (30, 55, 22),
        'shadow_color': (8, 12, 5),
        'wall_height': 0.38,
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
        'wall_top': (188, 198, 215),
        'wall_face_lit': (142, 158, 178),
        'wall_face_dark': (55, 72, 98),
        'wall_mortar': (85, 102, 128),
        'exterior': (38, 52, 72),
        'exterior_accent': (22, 32, 52),
        'exterior_detail': (50, 68, 90),
        'shadow_color': (15, 22, 40),
        'wall_height': 0.35,
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
        'wall_top': (75, 58, 48),
        'wall_face_lit': (58, 42, 34),
        'wall_face_dark': (22, 14, 10),
        'wall_mortar': (28, 18, 12),
        'exterior': (18, 6, 4),
        'exterior_accent': (55, 12, 4),
        'exterior_detail': (80, 20, 8),
        'shadow_color': (5, 2, 1),
        'wall_height': 0.42,
        'stone_density': 1.0,
        'moss_coverage': 0.18,
        'contrast': 40,
    },
}


# ---------------------------------------------------------------------------
# Continuous floor rendering
# ---------------------------------------------------------------------------

def render_floor_continuous(theme):
    """
    Generate the floor as one continuous image across the whole map.
    Uses map-scale Voronoi for stone blocks and multi-octave noise for variation.
    """
    pal = theme
    print("    Stone pattern...", end='', flush=True)
    cells, edges = continuous_voronoi(PX_H, PX_W, density=pal['stone_density'], seed=42)
    print(" done")

    print("    Surface noise...", end='', flush=True)
    surface = continuous_noise(PX_H, PX_W, scale=1.2, octaves=5, seed=100)
    fine_noise = continuous_noise(PX_H, PX_W, scale=4.0, octaves=3, seed=101)
    print(" done")

    base = np.array(pal['floor_base'], dtype=np.float64)
    accent = np.array(pal['floor_accent'], dtype=np.float64)
    mortar = np.array(pal['mortar'], dtype=np.float64)
    contrast = pal['contrast']

    img = np.zeros((PX_H, PX_W, 4), dtype=np.float64)

    # Stone cells: blend base/accent by cell value
    t = cells
    for c in range(3):
        img[:, :, c] = base[c] * t + accent[c] * (1 - t)
    img[:, :, 3] = 255.0

    # Surface variation (large scale)
    var_large = (surface - 0.5) * contrast
    # Fine detail
    var_fine = (fine_noise - 0.5) * (contrast * 0.4)
    for c in range(3):
        img[:, :, c] = np.clip(img[:, :, c] + var_large + var_fine, 0, 255)

    # Mortar/grout in Voronoi edges
    mortar_mask = np.clip(1.0 - edges * 3.5, 0, 1) ** 1.5
    for c in range(3):
        img[:, :, c] = img[:, :, c] * (1 - mortar_mask * 0.8) + mortar[c] * mortar_mask * 0.8

    return img


def render_floor_overlays(theme, wall_dist):
    """
    Generate continuous overlay layers: moss/frost/ember, stains, cracks.
    These are alpha layers that composite on top of the base floor.
    wall_dist: distance field from walls (for placing moss near edges).
    """
    pal = theme
    layers = []

    # --- Moss / frost / ember patches (multiple scales) ---
    print("    Moss/detail patches...", end='', flush=True)
    for i, moss_color in enumerate(pal['moss_colors']):
        color = np.array(moss_color, dtype=np.float64)
        n = continuous_noise(PX_H, PX_W, scale=0.6 + i * 0.3, octaves=3, seed=200 + i * 37)
        detail = continuous_noise(PX_H, PX_W, scale=2.5 + i, octaves=2, seed=210 + i * 37)

        threshold = 1.0 - pal['moss_coverage']
        mask = np.clip((n - threshold) / (1.0 - threshold + 1e-8), 0, 1)
        # Ragged edges from detail noise
        mask *= (0.6 + 0.4 * detail)
        # Moss prefers wall proximity (within 2 tiles)
        if wall_dist is not None:
            proximity = np.clip(1.0 - wall_dist / (TILE * 2.5), 0, 1) ** 0.5
            mask *= (0.3 + 0.7 * proximity)

        alpha = np.clip(mask * 0.7, 0, 1)
        layer = np.zeros((PX_H, PX_W, 4), dtype=np.float64)
        for c in range(3):
            layer[:, :, c] = color[c]
        layer[:, :, 3] = alpha * 255
        layers.append(layer)
    print(" done")

    # --- Stain patches (large, subtle) ---
    print("    Stains...", end='', flush=True)
    stain_color = np.array(pal['stain'], dtype=np.float64)
    stain_n = continuous_noise(PX_H, PX_W, scale=0.4, octaves=4, seed=300)
    stain_mask = np.clip(stain_n * 1.8 - 0.6, 0, 1) ** 1.2
    layer = np.zeros((PX_H, PX_W, 4), dtype=np.float64)
    for c in range(3):
        layer[:, :, c] = stain_color[c]
    layer[:, :, 3] = stain_mask * 0.35 * 255
    layers.append(layer)
    print(" done")

    # --- Crack network ---
    print("    Cracks...", end='', flush=True)
    _, crack_edges = continuous_voronoi(PX_H, PX_W, density=1.5, seed=400)
    cracks = np.clip(1.0 - crack_edges * 5.0, 0, 1) ** 3
    # Sparsify: only some areas have visible cracks
    crack_mask_n = continuous_noise(PX_H, PX_W, scale=0.5, octaves=2, seed=401)
    cracks *= np.clip(crack_mask_n * 2.5 - 1.0, 0, 1)
    layer = np.zeros((PX_H, PX_W, 4), dtype=np.float64)
    for c in range(3):
        layer[:, :, c] = pal['mortar'][c] * 0.6
    layer[:, :, 3] = cracks * 0.5 * 255
    layers.append(layer)
    print(" done")

    return layers


# ---------------------------------------------------------------------------
# Wall-proximity AO (continuous, not per-tile)
# ---------------------------------------------------------------------------

def render_ao(wall_dist, floor_mask):
    """
    Continuous ambient occlusion from wall distance field.
    Darkens floor pixels near walls with smooth falloff.
    """
    ao_radius = TILE * 0.6
    ao = np.zeros((PX_H, PX_W), dtype=np.float64)
    ao[floor_mask] = np.clip(1.0 - wall_dist[floor_mask] / ao_radius, 0, 1) ** 1.8
    return ao


# ---------------------------------------------------------------------------
# Shadow casting
# ---------------------------------------------------------------------------

def compute_shadows(theme):
    """
    Map-aware shadow casting. Light from NW, walls cast shadows SE.
    Shadows stop when they hit another wall.
    """
    wall_h_frac = theme['wall_height']
    shadow_len = int(TILE * wall_h_frac * 1.2)
    shadow = np.zeros((PX_H, PX_W), dtype=np.float32)

    shadow_color = theme['shadow_color']

    for r in range(ROWS):
        for c in range(COLS):
            if not is_wall(r, c):
                continue

            x0 = c * TILE
            y0 = r * TILE

            # South edge shadow (onto floor below)
            if is_floor(r + 1, c):
                for dy in range(shadow_len):
                    # Check if we've hit another wall
                    check_r = r + 1 + (dy // TILE)
                    if check_r < ROWS and is_wall(check_r + 1, c):
                        # Don't shadow past next wall
                        pass
                    t = 1.0 - dy / shadow_len
                    t = t ** 1.3
                    py = y0 + TILE + dy
                    if py >= PX_H:
                        break
                    # Check this pixel row is still floor
                    pr = py // TILE
                    if not is_floor(pr, c):
                        break
                    dx_spread = int(dy * 0.2)
                    for px in range(max(0, x0 - 1), min(x0 + TILE + dx_spread, PX_W)):
                        pc = px // TILE
                        if is_floor(pr, pc):
                            shadow[py, px] = max(shadow[py, px], t * 0.5)

            # East edge shadow (onto floor to right)
            if is_floor(r, c + 1):
                for dx in range(shadow_len):
                    t = 1.0 - dx / shadow_len
                    t = t ** 1.3
                    px = x0 + TILE + dx
                    if px >= PX_W:
                        break
                    pc = px // TILE
                    if not is_floor(r, pc):
                        break
                    dy_spread = int(dx * 0.2)
                    for py in range(max(0, y0 - 1), min(y0 + TILE + dy_spread, PX_H)):
                        pr = py // TILE
                        if is_floor(pr, pc):
                            shadow[py, px] = max(shadow[py, px], t * 0.4)

            # SE diagonal shadow
            if is_floor(r + 1, c + 1) and (is_wall(r + 1, c) or is_wall(r, c + 1)):
                for d in range(shadow_len):
                    t = 1.0 - d / shadow_len
                    t = t ** 2.0
                    py = y0 + TILE + d
                    px = x0 + TILE + d
                    if py >= PX_H or px >= PX_W:
                        break
                    pr, pc = py // TILE, px // TILE
                    if is_floor(pr, pc):
                        for s in range(max(1, shadow_len // 6)):
                            spy = min(py + s, PX_H - 1)
                            spx = min(px + s, PX_W - 1)
                            shadow[spy, spx] = max(shadow[spy, spx], t * 0.35)

    # Gaussian blur for soft edges
    shadow_img = Image.fromarray((shadow * 255).astype(np.uint8), 'L')
    shadow_img = shadow_img.filter(ImageFilter.GaussianBlur(radius=TILE // 6))
    return np.array(shadow_img, dtype=np.float32) / 255.0


# ---------------------------------------------------------------------------
# 2.5D Wall rendering (continuous textures)
# ---------------------------------------------------------------------------

def render_walls(canvas_arr, theme):
    """
    Render 2.5D walls directly into the canvas array.
    Wall tops get stone texture. South/east faces get depth-shaded stone.
    Wall edges have slight irregularity.
    """
    pal = theme
    face_h = max(4, int(TILE * pal['wall_height']))
    face_w = face_h

    print("    Wall top texture...", end='', flush=True)
    # Continuous stone for wall tops
    top_cells, top_edges = continuous_voronoi(PX_H, PX_W, density=0.9, seed=500)
    top_surface = continuous_noise(PX_H, PX_W, scale=1.5, octaves=3, seed=501)
    top_color = np.array(pal['wall_top'], dtype=np.float64)
    top_mortar = np.array(pal['wall_mortar'], dtype=np.float64)

    wall_top_img = np.zeros((PX_H, PX_W, 3), dtype=np.float64)
    t = top_cells
    for c in range(3):
        wall_top_img[:, :, c] = top_color[c] * (0.85 + 0.15 * t)
    var = (top_surface - 0.5) * 20
    for c in range(3):
        wall_top_img[:, :, c] = np.clip(wall_top_img[:, :, c] + var, 0, 255)
    # Mortar
    m = np.clip(1.0 - top_edges * 4.0, 0, 1) ** 1.5
    for c in range(3):
        wall_top_img[:, :, c] = wall_top_img[:, :, c] * (1 - m * 0.7) + top_mortar[c] * m * 0.7
    print(" done")

    # Apply wall tops
    for r in range(ROWS):
        for c in range(COLS):
            if not is_wall(r, c):
                continue
            y0, y1 = r * TILE, (r + 1) * TILE
            x0, x1 = c * TILE, (c + 1) * TILE
            canvas_arr[y0:y1, x0:x1, :3] = wall_top_img[y0:y1, x0:x1]
            canvas_arr[y0:y1, x0:x1, 3] = 255

    print("    Wall faces...", end='', flush=True)
    # Edge noise for wall face irregularity
    edge_noise = continuous_noise(PX_H, PX_W, scale=3.0, octaves=2, seed=510)

    face_lit = np.array(pal['wall_face_lit'], dtype=np.float64)
    face_dark = np.array(pal['wall_face_dark'], dtype=np.float64)
    face_mortar = np.array(pal['wall_mortar'], dtype=np.float64)

    # South faces: draw onto floor cells below wall cells
    for r in range(ROWS):
        for c in range(COLS):
            if not is_wall(r, c):
                continue
            if not is_floor(r + 1, c):
                continue
            # Also skip if this wall cell is actually a corridor opening
            # (i.e., floor on both north and south = not really a wall face)

            x0 = c * TILE
            y_start = (r + 1) * TILE

            for dy in range(face_h):
                py = y_start + dy
                if py >= PX_H:
                    break
                # Gradient: lit at top, dark at bottom
                t = (dy / face_h) ** 0.55
                for px in range(x0, min(x0 + TILE, PX_W)):
                    # Edge irregularity: vary the face height per pixel
                    local_h = face_h + int((edge_noise[py, px] - 0.5) * 6)
                    if dy > local_h:
                        continue
                    color = face_lit * (1 - t) + face_dark * t
                    # Add stone texture from top cells/edges
                    stone_var = (top_cells[py, px] - 0.5) * 15
                    mortar_t = np.clip(1.0 - top_edges[py, px] * 4.0, 0, 1) ** 2
                    for ch in range(3):
                        val = color[ch] + stone_var
                        val = val * (1 - mortar_t * 0.5) + face_mortar[ch] * mortar_t * 0.5
                        canvas_arr[py, px, ch] = np.clip(val, 0, 255)
                    canvas_arr[py, px, 3] = 255

    # East faces
    for r in range(ROWS):
        for c in range(COLS):
            if not is_wall(r, c):
                continue
            if not is_floor(r, c + 1):
                continue

            y0 = r * TILE
            x_start = (c + 1) * TILE

            for dx in range(face_w):
                px = x_start + dx
                if px >= PX_W:
                    break
                t = (dx / face_w) ** 0.55
                for py in range(y0, min(y0 + TILE, PX_H)):
                    local_w = face_w + int((edge_noise[py, px] - 0.5) * 6)
                    if dx > local_w:
                        continue
                    # East face is dimmer (less direct light)
                    color = (face_lit * 0.85) * (1 - t) + face_dark * t
                    stone_var = (top_cells[py, px] - 0.5) * 12
                    mortar_t = np.clip(1.0 - top_edges[py, px] * 4.0, 0, 1) ** 2
                    for ch in range(3):
                        val = color[ch] + stone_var
                        val = val * (1 - mortar_t * 0.5) + face_mortar[ch] * mortar_t * 0.5
                        canvas_arr[py, px, ch] = np.clip(val, 0, 255)
                    canvas_arr[py, px, 3] = 255

    # SE corner shadow pieces
    for r in range(ROWS):
        for c in range(COLS):
            if not is_wall(r, c):
                continue
            if is_floor(r + 1, c) and is_floor(r, c + 1):
                x_start = (c + 1) * TILE
                y_start = (r + 1) * TILE
                for dy in range(min(face_h, PX_H - y_start)):
                    for dx in range(min(face_w, PX_W - x_start)):
                        t = min(1.0, (dx / face_w + dy / face_h) / 1.2)
                        alpha = (1.0 - t) ** 1.5 * 0.7
                        py = y_start + dy
                        px = x_start + dx
                        # Darken existing pixel
                        for ch in range(3):
                            canvas_arr[py, px, ch] = np.clip(
                                canvas_arr[py, px, ch] * (1 - alpha), 0, 255)

    print(" done")


# ---------------------------------------------------------------------------
# Exterior rendering
# ---------------------------------------------------------------------------

def render_exterior(canvas_arr, theme):
    """Fill void cells with textured exterior."""
    print("    Exterior...", end='', flush=True)
    n1 = continuous_noise(PX_H, PX_W, scale=0.8, octaves=4, seed=600)
    n2 = continuous_noise(PX_H, PX_W, scale=3.0, octaves=3, seed=601)
    n3 = continuous_noise(PX_H, PX_W, scale=6.0, octaves=2, seed=602)

    base = np.array(theme['exterior'], dtype=np.float64)
    accent = np.array(theme['exterior_accent'], dtype=np.float64)
    detail = np.array(theme['exterior_detail'], dtype=np.float64)

    for r in range(ROWS):
        for c in range(COLS):
            if not is_void(r, c):
                continue
            y0, y1 = r * TILE, (r + 1) * TILE
            x0, x1 = c * TILE, (c + 1) * TILE

            t1 = n1[y0:y1, x0:x1]
            t2 = n2[y0:y1, x0:x1]
            t3 = n3[y0:y1, x0:x1]

            for ch in range(3):
                canvas_arr[y0:y1, x0:x1, ch] = np.clip(
                    base[ch] * t1 + accent[ch] * (1 - t1) +
                    (detail[ch] - base[ch]) * t2 * 0.3 +
                    (t3 - 0.5) * 15,
                    0, 255
                )
            canvas_arr[y0:y1, x0:x1, 3] = 255
    print(" done")


# ---------------------------------------------------------------------------
# Grid overlay
# ---------------------------------------------------------------------------

def render_grid(canvas, floor_mask):
    """Subtle grid lines on floor cells only."""
    draw = ImageDraw.Draw(canvas)
    for r in range(ROWS):
        for c in range(COLS):
            if not is_floor(r, c):
                continue
            x = c * TILE
            y = r * TILE
            # Bottom and right edges
            draw.line([(x, y + TILE - 1), (x + TILE - 1, y + TILE - 1)],
                      fill=(0, 0, 0, 25), width=1)
            draw.line([(x + TILE - 1, y), (x + TILE - 1, y + TILE - 1)],
                      fill=(0, 0, 0, 25), width=1)


# ---------------------------------------------------------------------------
# NW highlight (lit edge of walls)
# ---------------------------------------------------------------------------

def render_wall_highlight(canvas_arr, theme):
    """
    Subtle bright line on NW edges of walls (the lit side).
    Complements the SE shadow.
    """
    highlight = np.array(theme['wall_top'], dtype=np.float64) * 1.2
    highlight = np.clip(highlight, 0, 255)

    for r in range(ROWS):
        for c in range(COLS):
            if not is_wall(r, c):
                continue

            # North edge highlight (if cell above is floor)
            if is_floor(r - 1, c):
                y = r * TILE
                x0, x1 = c * TILE, (c + 1) * TILE
                for px in range(x0, min(x1, PX_W)):
                    for dy in range(2):
                        py = y + dy
                        if py < PX_H:
                            alpha = 0.3 * (1 - dy / 2)
                            for ch in range(3):
                                canvas_arr[py, px, ch] = np.clip(
                                    canvas_arr[py, px, ch] * (1 - alpha) +
                                    highlight[ch] * alpha, 0, 255)

            # West edge highlight (if cell left is floor)
            if is_floor(r, c - 1):
                x = c * TILE
                y0, y1 = r * TILE, (r + 1) * TILE
                for py in range(y0, min(y1, PX_H)):
                    for dx in range(2):
                        px = x + dx
                        if px < PX_W:
                            alpha = 0.3 * (1 - dx / 2)
                            for ch in range(3):
                                canvas_arr[py, px, ch] = np.clip(
                                    canvas_arr[py, px, ch] * (1 - alpha) +
                                    highlight[ch] * alpha, 0, 255)


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def render_preview(theme_name, output_path):
    theme = THEMES[theme_name]
    print(f"\n  Rendering {theme['name']}...")

    canvas_arr = np.zeros((PX_H, PX_W, 4), dtype=np.float64)

    # Exterior
    render_exterior(canvas_arr, theme)

    # Floor base (continuous)
    print("    Floor base...", end='', flush=True)
    floor_img = render_floor_continuous(theme)
    floor_mask = make_floor_mask()
    # Apply floor only to floor cells
    canvas_arr[floor_mask] = floor_img[floor_mask]
    print("    Applying floor mask... done")

    # Wall distance for AO and moss placement
    print("    Distance field...", end='', flush=True)
    try:
        wall_dist = wall_distance_field(floor_mask)
    except ImportError:
        # Fallback if scipy not available
        wall_dist = None
    print(" done")

    # Floor overlays (moss, stains, cracks) — continuous
    overlays = render_floor_overlays(theme, wall_dist)
    print("    Compositing overlays...", end='', flush=True)
    for layer in overlays:
        alpha = layer[:, :, 3:4] / 255.0
        mask3d = np.stack([floor_mask] * 4, axis=-1)
        blended = canvas_arr[:, :, :3] * (1 - alpha[:, :, :1]) + layer[:, :, :3] * alpha[:, :, :1]
        canvas_arr[:, :, :3] = np.where(np.stack([floor_mask]*3, axis=-1), blended, canvas_arr[:, :, :3])
    print(" done")

    # AO
    print("    Ambient occlusion...", end='', flush=True)
    if wall_dist is not None:
        ao = render_ao(wall_dist, floor_mask)
        ao_alpha = ao * 0.55
        for c in range(3):
            canvas_arr[:, :, c] = canvas_arr[:, :, c] * (1 - ao_alpha)
    print(" done")

    # Shadows
    print("    Shadows...", end='', flush=True)
    shadow_map = compute_shadows(theme)
    shadow_col = np.array(theme['shadow_color'], dtype=np.float64)
    for c in range(3):
        canvas_arr[:, :, c] = canvas_arr[:, :, c] * (1 - shadow_map * 0.7) + shadow_col[c] * shadow_map * 0.7
    print(" done")

    # Walls (2.5D)
    print("    Walls...", flush=True)
    render_walls(canvas_arr, theme)

    # NW highlight
    render_wall_highlight(canvas_arr, theme)

    # Convert to PIL for grid overlay
    canvas = Image.fromarray(np.clip(canvas_arr, 0, 255).astype(np.uint8), 'RGBA')

    # Grid
    render_grid(canvas, floor_mask)

    canvas.save(output_path)
    print(f"  -> {output_path}")
    return canvas


def main():
    preview_dir = Path(__file__).resolve().parent.parent.parent / 'assets' / 'previews'
    preview_dir.mkdir(parents=True, exist_ok=True)

    themes = ['jungle', 'ice', 'volcano']
    previews = []

    for name in themes:
        img = render_preview(name, preview_dir / f'{name}_preview.png')
        previews.append(img)

    # Comparison
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


if __name__ == '__main__':
    main()
